"""T-2725, AP-27: the admin-permissions Job narrows the Portal's client administration.

The pinned Keycloak runs in a container with a realm shaped like ours: fine-grained admin
permissions on, `portal-api` holding `view-clients` and `query-users`, the `edge` client, and an
App client carrying `managed-by: joinedcontext`. The Job's own script runs inside it, and the
test asks the Admin API as `portal-api` what it may still do. Measured on Keycloak 26.6.4 on
2026-10-06: before the script `portal-api` writes nothing; after it, it creates and changes App
clients and is refused every write on the edge, on itself and on Keycloak's own clients.
"""

import json
import shutil
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "components/keycloak/charts/admin-permissions/files/sync-admin-permissions.sh"
IMAGES = ROOT / "components/keycloak/images.yaml"
CLIENTS = ROOT / "components/portal/keycloak-clients.yaml"
REALM = "t2725"

# One worker runs the module, so one Keycloak serves its tests; a second worker's container started
# in the same second used to collide on the name (docker run exit 125, T-3897).
pytestmark = [
    pytest.mark.skipif(shutil.which("docker") is None, reason="no container runtime for Keycloak"),
    pytest.mark.xdist_group("docker-keycloak-admin-permissions"),
]


def portal_roles() -> list[str]:
    """The realm-management roles the deployment gives `portal-api`, as the realm import maps them."""
    return yaml.safe_load(CLIENTS.read_text())["portal-api"]["serviceAccountClientRoles"]["realm-management"]


def image() -> str:
    app = yaml.safe_load(IMAGES.read_text())["keycloak"]["app"]
    return f"{app['repository']}:{app['tag']}@{app['digest']}"


def call(base, method, path, token=None, body=None, form=None):
    data, headers = None, {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if body is not None:
        data, headers["Content-Type"] = json.dumps(body).encode(), "application/json"
    if form is not None:
        data = urllib.parse.urlencode(form).encode()
        headers["Content-Type"] = "application/x-www-form-urlencoded"
    request = urllib.request.Request(base + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            text = response.read().decode()
            return response.status, json.loads(text) if text.strip().startswith(("[", "{")) else text, response.headers
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode()[:300], error.headers


def token(base, realm, client, secret=None, user=None):
    form = {"client_id": client}
    if user:
        form.update(grant_type="password", username=user, password=user)
    else:
        form.update(grant_type="client_credentials", client_secret=secret)
    status, body, _ = call(base, "POST", f"/realms/{realm}/protocol/openid-connect/token", form=form)
    assert status == 200, body
    return body["access_token"]


@pytest.fixture(scope="module")
def keycloak():
    name = f"t2725-kc-{uuid.uuid4().hex[:12]}"
    subprocess.run(
        ["docker", "run", "-d", "--name", name, "-P", "-e", "KC_BOOTSTRAP_ADMIN_USERNAME=admin",
         "-e", "KC_BOOTSTRAP_ADMIN_PASSWORD=admin", image(), "start-dev"],
        check=True, capture_output=True,
    )
    try:
        port = subprocess.run(["docker", "port", name, "8080/tcp"], check=True, capture_output=True,
                              text=True).stdout.split("\n")[0].rsplit(":", 1)[1]
        base = f"http://127.0.0.1:{port}"
        deadline = time.time() + 180
        while time.time() < deadline:
            try:
                if call(base, "GET", "/realms/master")[0] == 200:
                    break
            except OSError:
                pass
            time.sleep(2)
        else:
            pytest.fail("Keycloak did not start within 3 minutes")
        subprocess.run(["docker", "cp", str(SCRIPT), f"{name}:/tmp/sync.sh"], check=True)
        yield name, base
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)


def realm(base, permissions=True):
    """The realm as our import leaves it, before the Job: returns the ids the checks use."""
    admin = token(base, "master", "admin-cli", user="admin")
    call(base, "DELETE", f"/admin/realms/{REALM}", admin)
    status, body, _ = call(base, "POST", "/admin/realms", admin,
                           {"realm": REALM, "enabled": True, "adminPermissionsEnabled": permissions})
    assert status == 201, body

    def client(client_id, attributes=None, service_account=False):
        status, body, headers = call(base, "POST", f"/admin/realms/{REALM}/clients", admin, {
            "clientId": client_id, "publicClient": False, "secret": f"{client_id}-secret",
            "serviceAccountsEnabled": service_account, "attributes": attributes or {}})
        assert status == 201, body
        return headers["Location"].rsplit("/", 1)[1]

    ids = {
        "portal": client("portal-api", service_account=True),
        "edge": client("edge"),
        "app": client("app-bikes", {"managed-by": "joinedcontext"}),
    }
    management = call(base, "GET", f"/admin/realms/{REALM}/clients?clientId=realm-management", admin)[1][0]["id"]
    roles = {r["name"]: r for r in call(base, "GET", f"/admin/realms/{REALM}/clients/{management}/roles", admin)[1]}
    account = call(base, "GET", f"/admin/realms/{REALM}/clients/{ids['portal']}/service-account-user", admin)[1]["id"]
    status, body, _ = call(base, "POST", f"/admin/realms/{REALM}/users/{account}/role-mappings/clients/{management}",
                           admin, [roles[name] for name in portal_roles()])
    assert status == 204, body
    # An App role a group holds, as the reconciler finds it on every cycle (T-3322).
    status, body, _ = call(base, "POST", f"/admin/realms/{REALM}/clients/{ids['app']}/roles", admin, {"name": "steward"})
    assert status == 201, body
    role = call(base, "GET", f"/admin/realms/{REALM}/clients/{ids['app']}/roles/steward", admin)[1]
    status, body, headers = call(base, "POST", f"/admin/realms/{REALM}/groups", admin, {"name": "stewards"})
    assert status == 201, body
    ids["group"] = headers["Location"].rsplit("/", 1)[1]
    status, body, _ = call(base, "POST", f"/admin/realms/{REALM}/groups/{ids['group']}/role-mappings/clients/{ids['app']}",
                           admin, [role])
    assert status == 204, body
    ids["admin"] = admin
    return ids


def sync(name):
    return subprocess.run(
        ["docker", "exec", "-e", "KEYCLOAK_URL=http://localhost:8080", "-e", "KEYCLOAK_USER=admin",
         "-e", "KEYCLOAK_PASSWORD=admin", "-e", f"REALM={REALM}", "-e", "PRINCIPAL=portal-api",
         name, "bash", "/tmp/sync.sh"],
        capture_output=True, text=True, timeout=180,
    )


def may(base, ids):
    portal = token(base, REALM, "portal-api", "portal-api-secret")
    admin = lambda method, path, body=None: call(base, method, f"/admin/realms/{REALM}{path}", portal, body)[0]
    return {
        "create an app client": admin("POST", "/clients", {"clientId": f"app-{time.time_ns()}",
                                                           "attributes": {"managed-by": "joinedcontext"}}),
        "change an app client": admin("PUT", f"/clients/{ids['app']}", {"clientId": "app-bikes", "description": "x",
                                                                          "attributes": {"managed-by": "joinedcontext"}}),
        "change the edge": admin("PUT", f"/clients/{ids['edge']}", {"clientId": "edge", "description": "x"}),
        "regenerate the edge secret": admin("POST", f"/clients/{ids['edge']}/client-secret"),
        "delete the edge": admin("DELETE", f"/clients/{ids['edge']}"),
        "change itself": admin("PUT", f"/clients/{ids['portal']}", {"clientId": "portal-api", "description": "x"}),
        "list clients": admin("GET", "/clients"),
        "read the users of an app role": admin("GET", f"/clients/{ids['app']}/roles/steward/users?max=1000"),
        "read the groups of an app role": admin("GET", f"/clients/{ids['app']}/roles/steward/groups?briefRepresentation=true&max=1000"),
        "read a group's members": admin("GET", f"/groups/{ids['group']}/members"),
    }


def test_the_portal_writes_its_own_clients_and_no_other(keycloak):
    name, base = keycloak
    ids = realm(base)
    before = may(base, ids)
    assert before["create an app client"] == 403 and before["change an app client"] == 403, before

    first = sync(name)
    assert first.returncode == 0, first.stderr
    assert "denied portal-api the writes on" in first.stdout
    after = may(base, ids)
    assert after == {
        "create an app client": 201,
        "change an app client": 204,
        "change the edge": 403,
        "regenerate the edge secret": 403,
        "delete the edge": 403,
        "change itself": 403,
        "list clients": 200,
        "read the users of an app role": 200,
        # Without `query-groups` this answered 403 on every reconcile cycle on dev (T-3322).
        "read the groups of an app role": 200,
        "read a group's members": 403,
    }, after

    # Idempotent: a second run rewrites the same four objects.
    again = sync(name)
    assert again.returncode == 0, again.stderr
    assert again.stdout.count("updated ") == 4, again.stdout

    # A client added by hand is the Portal's until the next run, and not after it.
    # A fresh admin token: master's admin-cli tokens live 60 s, and two runs of the Job on a busy
    # runner outlast the one `realm()` took, which answered 401 here (T-3179).
    admin = token(base, "master", "admin-cli", user="admin")
    status, _, headers = call(base, "POST", f"/admin/realms/{REALM}/clients", admin, {"clientId": "by-hand"})
    assert status == 201
    late = headers["Location"].rsplit("/", 1)[1]
    portal = token(base, REALM, "portal-api", "portal-api-secret")
    change = lambda: call(base, "PUT", f"/admin/realms/{REALM}/clients/{late}", portal,
                          {"clientId": "by-hand", "description": "x"})[0]
    assert change() == 204
    assert sync(name).returncode == 0
    assert change() == 403


def test_a_realm_without_admin_permissions_fails_the_run(keycloak):
    name, base = keycloak
    realm(base, permissions=False)
    run = sync(name)
    assert run.returncode != 0
    assert "adminPermissionsEnabled is off" in run.stderr, run.stderr


def test_the_job_links_the_marked_endpoint_scopes_to_the_hub(keycloak):
    """T-3324, EP-88: the Portal may not link a scope to `mcp-hub` (the deny covers it), so the
    Job links every marked `endpoint:` scope there as an optional scope and unlinks an unmarked
    one and any default `endpoint:` link; a scope outside the prefix is left as it is, and so is
    the realm's own default `endpoint:*`, which the import writes unmarked (T-4459, ADR-N-025 4b)."""
    name, base = keycloak
    ids = realm(base)
    admin = ids["admin"]
    status, body, headers = call(base, "POST", f"/admin/realms/{REALM}/clients", admin, {"clientId": "mcp-hub"})
    assert status == 201, body
    hub = headers["Location"].rsplit("/", 1)[1]

    def scope(scope_name, marked):
        attributes = {"include.in.token.scope": "true"}
        if marked:
            attributes["managed-by"] = "joinedcontext"
        status, body, headers = call(base, "POST", f"/admin/realms/{REALM}/client-scopes", admin,
                                     {"name": scope_name, "protocol": "openid-connect", "attributes": attributes})
        assert status == 201, body
        return headers["Location"].rsplit("/", 1)[1]

    scopes = {
        "endpoint:ours": scope("endpoint:ours", True),
        "endpoint:default": scope("endpoint:default", True),
        "endpoint:stray": scope("endpoint:stray", False),
        "endpoint:unlinked": scope("endpoint:unlinked", False),
        "weather": scope("weather", False),
        "endpoint:*": scope("endpoint:*", False),
    }
    for kind, scope_name in [("default", "endpoint:default"), ("optional", "endpoint:stray"), ("optional", "weather"),
                             ("default", "endpoint:*")]:
        status, body, _ = call(base, "PUT", f"/admin/realms/{REALM}/clients/{hub}/{kind}-client-scopes/{scopes[scope_name]}", admin)
        assert status == 204, body

    def links(kind):
        token_ = token(base, "master", "admin-cli", user="admin")
        status, body, _ = call(base, "GET", f"/admin/realms/{REALM}/clients/{hub}/{kind}-client-scopes", token_)
        assert status == 200, body
        return sorted(s["name"] for s in body if s["name"].startswith("endpoint:") or s["name"] == "weather")

    run = sync(name)
    assert run.returncode == 0, run.stderr
    assert links("optional") == ["endpoint:default", "endpoint:ours", "weather"], run.stdout
    assert links("default") == ["endpoint:*"], run.stdout
    assert "linked 2 of 2 endpoint: scopes to mcp-hub" in run.stdout, run.stdout
    assert "unlinked endpoint:stray from mcp-hub (optional)" in run.stdout, run.stdout
    assert "unlinked endpoint:default from mcp-hub (default)" in run.stdout, run.stdout

    # The Portal still may not link one itself: the deny stays on the hub.
    portal = token(base, REALM, "portal-api", "portal-api-secret")
    status, _, _ = call(base, "PUT", f"/admin/realms/{REALM}/clients/{hub}/optional-client-scopes/{scopes['endpoint:unlinked']}", portal)
    assert status == 403

    again = sync(name)
    assert again.returncode == 0, again.stderr
    assert "linked 0 of 2 endpoint: scopes to mcp-hub" in again.stdout, again.stdout
    assert "unlinked" not in again.stdout, again.stdout
    assert again.stdout.count("updated ") == 4, again.stdout


def test_the_job_links_the_marked_endpoint_scopes_to_jcctl_and_no_star(keycloak):
    """T-5614, PF-46: `jcctl token --resource` asks for one Endpoint's `endpoint:{slug}` scope, so
    the Job links every marked one to `jcctl` as optional, and unlinks a stray, a default link and
    `endpoint:*` there too: no jcctl token names every Endpoint, and none carries a scope the
    person did not ask for."""
    name, base = keycloak
    ids = realm(base)
    admin = ids["admin"]
    status, body, headers = call(base, "POST", f"/admin/realms/{REALM}/clients", admin,
                                 {"clientId": "jcctl", "publicClient": True})
    assert status == 201, body
    cli = headers["Location"].rsplit("/", 1)[1]

    def scope(scope_name, marked):
        attributes = {"include.in.token.scope": "true"}
        if marked:
            attributes["managed-by"] = "joinedcontext"
        status, body, headers = call(base, "POST", f"/admin/realms/{REALM}/client-scopes", admin,
                                     {"name": scope_name, "protocol": "openid-connect", "attributes": attributes})
        assert status == 201, body
        return headers["Location"].rsplit("/", 1)[1]

    scopes = {n: scope(n, marked) for n, marked in [
        ("endpoint:cli-ours", True), ("endpoint:cli-default", True), ("endpoint:cli-stray", False),
        ("endpoint:*", False)]}
    for kind, scope_name in [("default", "endpoint:cli-default"), ("optional", "endpoint:cli-stray"),
                             ("optional", "endpoint:*")]:
        status, body, _ = call(base, "PUT", f"/admin/realms/{REALM}/clients/{cli}/{kind}-client-scopes/{scopes[scope_name]}", admin)
        assert status == 204, body

    run = sync(name)
    assert run.returncode == 0, run.stderr
    admin = token(base, "master", "admin-cli", user="admin")
    links = {}
    for kind in ["default", "optional"]:
        status, body, _ = call(base, "GET", f"/admin/realms/{REALM}/clients/{cli}/{kind}-client-scopes", admin)
        assert status == 200, body
        links[kind] = sorted(s["name"] for s in body if s["name"].startswith("endpoint:"))
    assert links["default"] == [], run.stdout
    assert "endpoint:cli-ours" in links["optional"] and "endpoint:cli-default" in links["optional"], links
    assert "endpoint:cli-stray" not in links["optional"] and "endpoint:*" not in links["optional"], links
    assert "unlinked endpoint:* from jcctl (optional)" in run.stdout, run.stdout
    assert "unlinked endpoint:cli-default from jcctl (default)" in run.stdout, run.stdout

    again = sync(name)
    assert again.returncode == 0, again.stderr
    assert "unlinked" not in again.stdout, again.stdout


def claims(jwt):
    import base64
    payload = jwt.split(".")[1]
    return json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))


def test_the_portal_exchanges_a_connector_token_for_one_endpoint_once_the_job_ran(keycloak):
    """T-6063, API/14 §4.2: a connector's `jc_data_query` exchanges the person's token (a CIMD
    client's: public, no role scope, audience `portal-api` through the `connector` scope) as
    `portal-api` for audience `mcp-hub` and `gateway-hub endpoint:{slug}`. Keycloak refuses it
    `invalid_scope` until the Job links the marked `endpoint:` scope to `portal-api`; after the
    run the token names the hub alone, `azp` the Portal and the person, and an unmarked scope
    stays refused. The realm side mirrors components/portal/keycloak-clients.yaml and the
    `gateway-hub` scope of components/keycloak (pinned by tests/test_keycloak_clients.py)."""
    name, base = keycloak
    ids = realm(base)
    admin = ids["admin"]

    def post(path, body):
        status, answer, headers = call(base, "POST", f"/admin/realms/{REALM}{path}", admin, body)
        assert status == 201, answer
        return headers["Location"].rsplit("/", 1)[1]

    def audience(mapper, key, value):
        return {"name": mapper, "protocol": "openid-connect", "protocolMapper": "oidc-audience-mapper",
                "config": {key: value, "access.token.claim": "true"}}

    post("/clients", {"clientId": "mcp-hub", "publicClient": False, "secret": "mcp-hub-secret"})
    hub_scope = post("/client-scopes", {"name": "gateway-hub", "protocol": "openid-connect",
                                        "attributes": {"include.in.token.scope": "false"},
                                        "protocolMappers": [audience("hub", "included.client.audience", "mcp-hub")]})
    post("/client-scopes", {"name": "connector", "protocol": "openid-connect",
                            "attributes": {"include.in.token.scope": "true"},
                            "protocolMappers": [audience("door", "included.custom.audience", "portal-connectors"),
                                                audience("portal", "included.client.audience", "portal-api")]})
    for scope_name, marked in [("endpoint:ex-ours", True), ("endpoint:ex-stray", False)]:
        attributes = {"include.in.token.scope": "true"}
        if marked:
            attributes["managed-by"] = "joinedcontext"
        post("/client-scopes", {"name": scope_name, "protocol": "openid-connect", "attributes": attributes})
    post("/clients", {"clientId": "cimd", "publicClient": True, "directAccessGrantsEnabled": True,
                      "fullScopeAllowed": False, "defaultClientScopes": ["basic", "profile"],
                      "optionalClientScopes": ["connector"]})
    post("/users", {"username": "jana", "enabled": True, "email": "jana@example.org", "firstName": "Jana",
                    "lastName": "Example", "emailVerified": True,
                    "credentials": [{"type": "password", "value": "jana", "temporary": False}]})
    portal = ids["portal"]
    status, client, _ = call(base, "GET", f"/admin/realms/{REALM}/clients/{portal}", admin)
    assert status == 200, client
    client["attributes"]["standard.token.exchange.enabled"] = "true"
    status, body, _ = call(base, "PUT", f"/admin/realms/{REALM}/clients/{portal}", admin, client)
    assert status == 204, body
    status, body, _ = call(base, "PUT", f"/admin/realms/{REALM}/clients/{portal}/optional-client-scopes/{hub_scope}", admin)
    assert status == 204, body

    status, body, _ = call(base, "POST", f"/realms/{REALM}/protocol/openid-connect/token", form={
        "grant_type": "password", "client_id": "cimd", "username": "jana", "password": "jana",
        "scope": "openid connector"})
    assert status == 200, body
    subject = body["access_token"]
    assert "portal-api" in claims(subject)["aud"]

    def exchange(slug):
        return call(base, "POST", f"/realms/{REALM}/protocol/openid-connect/token", form={
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange", "client_id": "portal-api",
            "client_secret": "portal-api-secret", "subject_token": subject,
            "subject_token_type": "urn:ietf:params:oauth:token-type:access_token",
            "requested_token_type": "urn:ietf:params:oauth:token-type:access_token",
            "audience": "mcp-hub", "scope": f"gateway-hub endpoint:{slug}"})

    status, body, _ = exchange("ex-ours")
    assert status == 400 and "invalid_scope" in body, (status, body)

    run = sync(name)
    assert run.returncode == 0, run.stderr
    assert "endpoint: scopes to portal-api" in run.stdout, run.stdout

    status, body, _ = exchange("ex-ours")
    assert status == 200, body
    token_ = claims(body["access_token"])
    assert token_["aud"] in ("mcp-hub", ["mcp-hub"]), token_["aud"]
    assert token_["azp"] == "portal-api" and token_["preferred_username"] == "jana", token_
    assert "endpoint:ex-ours" in token_["scope"].split(), token_["scope"]
    status, body, _ = exchange("ex-stray")
    assert status == 400 and "invalid_scope" in body, (status, body)


def test_the_job_links_a_thousand_new_scopes_in_one_process(keycloak):
    """T-4474 (ADR-N-025 §4d): one kcadm JVM per link took a second each, so a fresh realm with
    10,000 Endpoint scopes never converged inside the Job's 900 s. The links go over one bash
    process now. T-5377: the test counts kcadm starts through a shim instead of racing the wall
    clock, which a host at load 60 stretched past a minute: a thousand links take a fixed few
    kcadm runs, the per-link loop a thousand."""
    name, base = keycloak
    ids = realm(base)
    admin = ids["admin"]
    status, body, headers = call(base, "POST", f"/admin/realms/{REALM}/clients", admin, {"clientId": "mcp-hub"})
    assert status == 201, body
    hub = headers["Location"].rsplit("/", 1)[1]
    for n in range(1000):
        if n % 200 == 0:
            admin = token(base, "master", "admin-cli", user="admin")
        status, body, _ = call(base, "POST", f"/admin/realms/{REALM}/client-scopes", admin, {
            "name": f"endpoint:s{n:05d}", "protocol": "openid-connect",
            "attributes": {"include.in.token.scope": "true", "managed-by": "joinedcontext"}})
        assert status == 201, body

    bin_ = "/opt/keycloak/bin"
    subprocess.run(["docker", "exec", "-u", "0", name, "sh", "-c",
                    (f"mv {bin_}/kcadm.sh {bin_}/kcadm-real.sh && "
                    f"printf '#!/bin/sh\\necho run >> /tmp/kcadm.runs\\nexec {bin_}/kcadm-real.sh \"$@\"\\n' > {bin_}/kcadm.sh && "
                    f"chmod 755 {bin_}/kcadm.sh && : > /tmp/kcadm.runs && chmod 666 /tmp/kcadm.runs")],
                   check=True, capture_output=True)
    run = sync(name)
    assert run.returncode == 0, run.stderr
    assert "linked 1000 of 1000 endpoint: scopes to mcp-hub" in run.stdout, run.stdout
    runs = subprocess.run(["docker", "exec", name, "wc", "-l", "/tmp/kcadm.runs"], check=True,
                          capture_output=True, text=True).stdout.split()[0]
    assert 0 < int(runs) < 50, f"{runs} kcadm runs for 1000 links"
    admin = token(base, "master", "admin-cli", user="admin")
    status, body, _ = call(base, "GET", f"/admin/realms/{REALM}/clients/{hub}/optional-client-scopes", admin)
    assert status == 200, body
    assert sum(s["name"].startswith("endpoint:s") for s in body) == 1000
