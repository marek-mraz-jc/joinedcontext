"""T-0270: the portal-api client must list the callback the Portal actually uses;
T-0248: the development profile seeds the DEMO.md users, production seeds none.

The Portal derives its redirect URI as `{JC_PORTAL_PUBLIC_URL}/api/v1/auth/callback`
(portal src/config.rs). Keycloak matches redirect URIs exactly, so a client listing any
other path turns every browser login into "Invalid parameter: redirect_uri".
"""

import base64
import json
import re

import pytest


@pytest.fixture(scope="module")
def realm_clients(rendered):
    for doc in rendered("local"):
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            realm = json.loads(base64.b64decode(doc["data"]["local.json"]))
            return {c["clientId"]: c for c in realm["clients"]}
    pytest.fail("no keycloak-config-cli realm Secret in the local render")


def test_portal_api_redirect_uri_is_the_portal_callback(realm_clients):
    uris = realm_clients["portal-api"]["redirectUris"]
    assert uris, "portal-api lists no redirect URI at all"
    assert all(uri.endswith("/api/v1/auth/callback") for uri in uris), uris


def test_portal_api_redirect_uris_are_exact(realm_clients):
    # Security line of T-0270: exact matches only, no wildcard on a confidential client.
    assert not any("*" in uri for uri in realm_clients["portal-api"]["redirectUris"])


def test_the_portal_clients_live_on_the_portal_host(realm_clients):
    """ADR-N-019: the Portal moved to portal.{host}; its own clients follow, or the fallback
    code flow of the backend and the UI's origin would still name the apex."""
    for client_id in ("portal-ui", "portal-api"):
        client = realm_clients[client_id]
        assert client["rootUrl"] == "https://portal.joinedcontext.test", client_id
        assert all(uri.startswith("https://portal.joinedcontext.test/") for uri in client["redirectUris"]), client_id
        assert client["webOrigins"] == ["https://portal.joinedcontext.test"], client_id


# --- ADR-N-019, AP-27: the `edge` client of the APISIX openid-connect plugin ------------------


def test_the_edge_client_logs_people_in_on_both_hosts(realm_clients):
    """One confidential client for the Portal host and the apps surface of the apex: the
    authorization code flow with PKCE, no service account, no password grant, no browser
    origin (the flow runs inside APISIX)."""
    edge = realm_clients["edge"]
    assert set(edge["redirectUris"]) == {"https://portal.joinedcontext.test/*", "https://joinedcontext.test/apps/*"}
    assert edge["publicClient"] is False and edge["bearerOnly"] is False
    assert edge["standardFlowEnabled"] is True and edge["implicitFlowEnabled"] is False
    assert edge["directAccessGrantsEnabled"] is False and edge["serviceAccountsEnabled"] is False
    assert edge["webOrigins"] == []
    assert edge["attributes"]["pkce.code.challenge.method"] == "S256"
    # "+" = the redirect URIs, so the post-logout landing on the Portal host is accepted.
    assert edge["attributes"]["post.logout.redirect.uris"] == "+"
    assert edge["secret"] == "$(CLIENT_SECRET_EDGE)", "the secret is substituted from the generated Secret"
    # The gateway's own name as an audience: the session reaches an endpoint approved after
    # the login (Architecture/12 §4); the Portal's audience stays beside it.
    audiences = {
        m["config"].get("included.custom.audience") or m["config"].get("included.client.audience")
        for m in edge["protocolMappers"]
        if m["protocolMapper"] == "oidc-audience-mapper"
    }
    assert {"portal-api", "context-gateway"} <= audiences, audiences


def test_only_the_edge_client_is_signed_rs256(realm_clients):
    """lua-resty-openidc verifies RS/HS signatures only, so the edge client's tokens are RS256
    by per-client override; the realm default stays ES256 for everyone else (T-0252, AR-11)."""
    edge = realm_clients["edge"]["attributes"]
    assert edge["access.token.signed.response.alg"] == "RS256"
    assert edge["id.token.signed.response.alg"] == "RS256"
    for client_id, client in realm_clients.items():
        if client_id == "edge":
            continue
        attributes = client.get("attributes") or {}
        assert not any(key.endswith(".signed.response.alg") for key in attributes), client_id


@pytest.mark.parametrize("env", ["local", "production"])
def test_the_rs256_key_is_active_below_the_es256_key(rendered, env):
    """The realm signs ES256 by default and keeps an active RS256 key at a lower priority for
    the one client that asks for it (ADR-N-019); the HS512 key stays disabled."""
    realm = realm_of(rendered, env)
    assert realm["defaultSignatureAlgorithm"] == "ES256"
    keys = {k["name"]: k["config"] for k in realm["components"]["org.keycloak.keys.KeyProvider"]}
    assert keys["ecdsa-generated"]["active"] == ["true"] and keys["ecdsa-generated"]["enabled"] == ["true"]
    assert keys["rsa-generated"]["active"] == ["true"] and keys["rsa-generated"]["enabled"] == ["true"]
    assert keys["rsa-generated"]["algorithm"] == ["RS256"]
    assert int(keys["rsa-generated"]["keySize"][0]) >= 3072, "BSI TR-02102-1: RSA moduli of 3000 bits or more"
    assert int(keys["rsa-generated"]["priority"][0]) < int(keys["ecdsa-generated"]["priority"][0])
    assert keys["hmac-generated"]["active"] == ["false"] and keys["hmac-generated"]["enabled"] == ["false"]


def realm_of(rendered, env):
    for doc in rendered(env):
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            return json.loads(base64.b64decode(next(iter(doc["data"].values()))))
    pytest.fail(f"no keycloak-config-cli realm Secret in the {env} render")


def human_users(realm):
    """The realm users that are people; a service account is a user Keycloak owns."""
    return {
        u["username"]: u
        for u in realm["users"]
        if "serviceAccountClientId" not in u
    }


SHARING_PEOPLE = {
    "demo.outsider@hel.fi": [],
    "demo.neighbour@hel.fi": ["/sharing-other"],
    "demo.member@hel.fi": ["/sharing"],
    "demo.author@hel.fi": ["/sharing"],
    "demo.bearer@hel.fi": [],
}


def test_every_seeded_person_holds_the_realm_default_roles(rendered):
    """The people Job creates each person by a partial import, which grants no default roles,
    and keycloak-config-cli keeps only the roles a user lists. Without the default composite a
    person holds no offline_access, and a sign-in that asks for a refresh token (claude.ai, every
    CIMD connector) fails at code-to-token with not_allowed (T-6301)."""
    realm = realm_of(rendered, "dev")
    people = human_users(realm)
    assert people
    for name, user in people.items():
        assert user["realmRoles"][0] == f"default-roles-{realm['realm']}", name
    assert "offline_access" in realm["defaultRoles"]


def test_development_profile_seeds_the_demo_users(rendered):
    realm = realm_of(rendered, "dev")
    users = {
        name: {**u, "realmRoles": u["realmRoles"][1:]} for name, u in human_users(realm).items()
    }
    # A Yellow change needs an approver who is not its author (CC-34), so the demo has two —
    # and a fourth person who proposes and decides nothing, so the plain self-approval refusal
    # can be played by somebody it applies to (T-2231).
    assert set(users) == {
        "demo.steward@hel.fi",
        "demo.viewer@hel.fi",
        "demo.approver@hel.fi",
        "demo.editor@hel.fi",
        "demo.janitor@hel.fi",
        "demo.probe@hel.fi",
        # Mesto Žilina's organization administrator (ADR-N-058, T-5242).
        "demo.zilina@hel.fi",
        # Ajuntament de Barcelona's (T-5260).
        "demo.barcelona@hel.fi",
        # Hlavné mesto SR Bratislava's (T-5271).
        "demo.bratislava@hel.fi",
        # Organization members with no project (ADR-N-067, T-6408).
        "demo.orgmember@hel.fi",
        "demo.analyst@hel.fi",
        *SHARING_PEOPLE,
        # The person the owner presents as on dev, stage and prod (T-6542, tests/test_presenter.py).
        "presenter@hel.fi",
    }
    assert users["demo.steward@hel.fi"]["realmRoles"] == ["portal-approver"]
    assert users["demo.zilina@hel.fi"]["realmRoles"] == ["portal-approver"]
    assert users["demo.zilina@hel.fi"]["groups"] == ["/platform-readers", "/zilina"]
    assert users["demo.barcelona@hel.fi"]["realmRoles"] == ["portal-approver"]
    assert users["demo.barcelona@hel.fi"]["groups"] == ["/platform-readers", "/barcelona"]
    assert users["demo.bratislava@hel.fi"]["realmRoles"] == ["portal-approver"]
    assert users["demo.bratislava@hel.fi"]["groups"] == ["/platform-readers", "/bratislava"]
    assert users["demo.approver@hel.fi"]["realmRoles"] == ["portal-approver"]
    assert users["demo.viewer@hel.fi"]["realmRoles"] == [], "demo.viewer is read only"
    assert users["demo.editor@hel.fi"]["realmRoles"] == [], "demo.editor approves nothing"
    # The residue sweep's approver (T-2627): its RoleBinding is what it may do, and no project
    # group admits it to the gateway, so it reads no context data.
    assert users["demo.janitor@hel.fi"]["realmRoles"] == []
    assert not {"/helsinki", "/banskabystrica", "helsinki", "banskabystrica"} & set(
        users["demo.janitor@hel.fi"].get("groups", [])
    )
    # The App probe (T-2795, AP-136) decides nothing; its groups are the ones Apps admit by.
    assert users["demo.probe@hel.fi"]["realmRoles"] == []
    # The App sharing matrix's people (T-4911): no realm role, and out of `platform-readers`, whose
    # organization-wide viewer binding would make each a reader of every project.
    for name, groups in SHARING_PEOPLE.items():
        assert users[name]["realmRoles"] == [], name
        assert users[name]["groups"] == groups, name
    assert "portal-approver" in {r["name"] for r in realm["roles"]["realm"]}
    assert realm["registrationEmailAsUsername"] is True
    for user in users.values():
        assert user["email"].endswith("@hel.fi")
        # keycloak-config-cli refuses a user whose username is not the email while the realm
        # uses email as username, and the import aborts before it sets any password (T-0366).
        assert user["username"] == user["email"]
        assert user["requiredActions"] == [], "a required action would block the demo login"
        if user["username"] == "presenter@hel.fi":
            # The owner's dev password, copied hashed by scripts/presenter-secret.sh (T-6542).
            assert "credentials" not in user
            continue
        credential, *otp = user["credentials"]
        # Security line of T-0248: the password is a per-cluster Secret substituted at import
        # time, never a literal in the rendered realm.
        assert credential["value"].startswith("$(USER_PASSWORD_") and credential["temporary"] is False
        # Dev's approvers sign in with the password alone (T-5776, owner 2026-10-26): no seeded
        # one-time code; smartcity and production keep T-5120 (test_keycloak_login_hardening.py).
        assert otp == []


def test_demo_passwords_are_generated_per_cluster(rendered):
    secrets = {d["metadata"]["name"]: d for d in rendered("dev")
               if d.get("kind") == "Secret" and d["metadata"]["name"].startswith("keycloak-user-")}
    assert set(secrets) == {
        "keycloak-user-demo-steward",
        "keycloak-user-demo-viewer",
        "keycloak-user-demo-approver",
        "keycloak-user-demo-editor",
        "keycloak-user-demo-janitor",
        "keycloak-user-demo-probe",
        "keycloak-user-demo-zilina",
        "keycloak-user-demo-barcelona",
        "keycloak-user-demo-bratislava",
        "keycloak-user-demo-orgmember",
        "keycloak-user-demo-analyst",
        *("keycloak-user-" + name.split("@")[0].replace(".", "-") for name in SHARING_PEOPLE),
    }
    for secret in secrets.values():
        assert secret["metadata"]["annotations"]["helm.sh/resource-policy"] == "keep"
        # No TOTP seed on dev (T-5776): its approvers sign in without a one-time code.
        assert sorted(secret["data"]) == ["password"]


def test_production_seeds_no_human_users(rendered):
    assert human_users(realm_of(rendered, "production")) == {}
    assert not [d for d in rendered("production")
                if d.get("kind") == "Secret" and d["metadata"]["name"].startswith("keycloak-user-")]


# T-0411, AP-27: the reconciler creates the confidential client `app-{name}` of every app on
# demand. It authenticates as the service account of the Portal's own client, so that account
# is the one identity in the realm with an Admin API role. T-2725: the role is the read-only
# `view-clients`; the writes come from the realm's fine-grained admin permissions, granted on
# the clients it manages and denied on every other one.
@pytest.mark.parametrize("env", ["local", "production", "dev"])
def test_no_audience_mapper_puts_a_resource_into_an_id_token(rendered, env):
    """F-14 (T-6662): an ID token never names a resource. The gateway refuses a token whose `typ`
    is not `Bearer` (jc-core check_signature); this keeps the realm from minting one that would
    need that refusal: every audience mapper writes the access token alone."""
    realm = realm_of(rendered, env)
    holders = list(realm.get("clients", [])) + list(realm.get("clientScopes", []))
    mappers = [
        (holder.get("clientId") or holder.get("name"), mapper)
        for holder in holders
        for mapper in holder.get("protocolMappers", [])
        if mapper.get("protocolMapper") == "oidc-audience-mapper"
    ]
    assert mappers, "the realm has no audience mapper; the walk found nothing to check"
    for holder, mapper in mappers:
        assert mapper.get("config", {}).get("id.token.claim") == "false", (holder, mapper["name"])


@pytest.mark.parametrize("env", ["local", "production", "dev"])
def test_the_portal_service_account_may_manage_clients(rendered, env):
    realm = realm_of(rendered, env)
    accounts = {u["serviceAccountClientId"]: u
                for u in realm["users"] if "serviceAccountClientId" in u}
    assert "portal-api" in accounts, "the reconciler could not create an app's client"
    assert accounts["portal-api"]["clientRoles"] == {
        "realm-management": ["view-clients", "query-users", "query-groups"]
    }
    assert accounts["portal-api"]["username"] == "service-account-portal-api", (
        "keycloak names a service account after its client; another name creates a second user"
    )
    client = {c["clientId"]: c for c in realm["clients"]}["portal-api"]
    assert client["serviceAccountsEnabled"] is True, (
        "the role is mapped onto an account the client does not have"
    )


@pytest.mark.parametrize("env", ["local", "production", "dev"])
def test_no_other_client_holds_an_admin_api_role(rendered, env):
    """Security line of T-0411 and T-0866: only the Portal reaches the Admin API, and each of
    its two clients holds the narrowest role for the one job it does.

    `portal-api` creates an app's client; `portal-reconciler` writes the memberships of the
    groups the repository declares (PF-63) and nothing else — the login client must not gain
    that right. Every other workload is a service account with its own audience-bound token and
    no reason to reach the Admin API; another holder would make the blast radius of any one
    leaked client secret the whole realm.

    T-3022 (AP-113): `portal-api` also holds `query-users`, read-only, because Keycloak lists a
    client role's holders only to a caller that may view the client. It never gains
    `manage-users` (the mapping writes stay on `portal-reconciler`).

    T-2725: `portal-api` no longer holds `manage-clients`, which let it rewrite or delete any
    client of the realm, the edge's included. It holds `view-clients`, which reads what
    `manage-clients` already read (every client, its secret too; that read goes when the
    confidential platform clients move off shared secrets, T-2868), and its writes come from
    the realm's fine-grained admin permissions on the clients it manages alone.

    T-3322: `portal-api` also holds `query-groups`, read-only, for the groups that hold an App
    client role; it reads no group's members and writes nothing."""
    realm = realm_of(rendered, env)
    holders = {
        u["serviceAccountClientId"]: u["clientRoles"]["realm-management"]
        for u in realm["users"]
        if u.get("clientRoles", {}).get("realm-management")
    }
    assert set(holders) == {"portal-api", "portal-reconciler"}, holders
    assert sorted(holders["portal-api"]) == ["query-groups", "query-users", "view-clients"], holders
    assert sorted(holders["portal-reconciler"]) == ["manage-users", "query-groups"], holders


@pytest.mark.parametrize("env", ["local", "production", "dev"])
def test_the_reconciler_token_names_the_gateway_and_no_other_resource(rendered, env):
    """T-4866: the reconciler writes declared Subscriptions, drift repairs and the catalogue
    through the Context Gateway, which verifies the `context-gateway` audience; without it every
    write answered 404 and no Subscription ever reached a space. The one audience it gains is the
    gateway's: no broker, no jc-functions, no Portal API."""
    realm = realm_of(rendered, env)
    client = {c["clientId"]: c for c in realm["clients"]}["portal-reconciler"]
    audiences = sorted(
        m["config"]["included.custom.audience"]
        for m in client.get("protocolMappers", [])
        if m["protocolMapper"] == "oidc-audience-mapper"
    )
    assert audiences == ["context-gateway"], audiences
    assert not client.get("optionalClientScopes", []) or not any(
        s.startswith("context-broker-") for s in client["optionalClientScopes"]
    ), client["optionalClientScopes"]


@pytest.mark.parametrize("env", ["local", "production", "dev"])
def test_the_group_reconciler_client_has_no_login_flow(rendered, env):
    """PF-63: the credential that writes group memberships logs nobody in — no code flow, no
    password grant, a service account and nothing else."""
    realm = realm_of(rendered, env)
    client = {c["clientId"]: c for c in realm["clients"]}["portal-reconciler"]
    assert client["serviceAccountsEnabled"] is True
    assert client["standardFlowEnabled"] is False
    assert client["directAccessGrantsEnabled"] is False
    assert client["publicClient"] is False


LABEL = re.compile(r"^(([A-Za-z0-9][-A-Za-z0-9_.]*)?[A-Za-z0-9])?$")


@pytest.mark.parametrize("env", ["local", "production", "dev"])
def test_every_rendered_label_value_is_a_valid_label(rendered, env):
    """A digest in `image.tag` reaches `app.kubernetes.io/version`, and the API server
    refuses the upgrade: `@` and `:` are not label characters. helm template and
    kubeconform both accept it, so only this check stands between a digest pin and a
    dev-apply that cannot patch the StatefulSet (T-0368)."""
    for doc in rendered(env):
        for where, labels in (
            (doc.get("metadata", {}).get("name", "?"), doc.get("metadata", {}).get("labels") or {}),
        ):
            for key, value in labels.items():
                assert len(value) <= 63, f"{doc.get('kind')}/{where}: label {key} is longer than 63 characters"
                assert LABEL.match(value), f"{doc.get('kind')}/{where}: label {key}={value!r} is not a valid label value"


def test_mcp_mobile_is_a_public_pkce_client(realm_clients):
    """T-0336 / PF-45: a phone reaching an endpoint's MCP route logs in with this client.

    It lives on a device somebody can lose, so it carries no secret and PKCE S256 is
    mandatory rather than optional; the audience (RFC 8707 `resource`) is what binds a
    token to one endpoint, which is why there is one client and not one per endpoint.
    """
    client = realm_clients["mcp-mobile"]
    assert client["publicClient"] is True
    assert "secret" not in client, "a public client must not carry a secret"
    assert client["attributes"]["pkce.code.challenge.method"] == "S256"
    assert client["standardFlowEnabled"] is True
    assert client["implicitFlowEnabled"] is False
    assert client["directAccessGrantsEnabled"] is False
    assert client["serviceAccountsEnabled"] is False


def test_mcp_mobile_lists_only_the_known_mobile_callbacks(realm_clients):
    # Security line of T-0336: dynamic client registration stays off, so this list is the
    # whole set of places an authorization code may be delivered. A wildcard here would let
    # anybody who can host a page collect codes for a client that needs no secret.
    assert set(realm_clients["mcp-mobile"]["redirectUris"]) == {
        "https://claude.ai/api/mcp/auth_callback",
        "https://claude.com/api/mcp/auth_callback",
        "https://chatgpt.com/connector_platform_oauth_redirect",
        "https://platform.openai.com/chatkit-oauth-callback",
    }


def test_mcp_local_takes_a_loopback_callback_on_any_port_and_nothing_else(realm_clients):
    """T-5611, RFC 8252: Claude Code and IDE clients receive the code on a local port that changes
    per login. Keycloak 26 ignores the port of a registered loopback URI and matches scheme, host
    and path exactly (measured on 26.8.0: :42240/callback 200, :42240/other, https://localhost
    and another host 400), so two exact URIs cover it with no wildcard."""
    client = realm_clients["mcp-local"]
    assert set(client["redirectUris"]) == {"http://localhost/callback", "http://127.0.0.1/callback"}
    assert client["publicClient"] is True and "secret" not in client
    assert client["attributes"]["pkce.code.challenge.method"] == "S256"
    assert client["standardFlowEnabled"] is True
    assert not (client["implicitFlowEnabled"] or client["directAccessGrantsEnabled"] or client["serviceAccountsEnabled"])
    # A token on a laptop dies with the session: short access tokens, no offline refresh token.
    assert int(client["attributes"]["access.token.lifespan"]) <= 300
    assert int(client["attributes"]["client.session.max.lifespan"]) <= 8 * 3600
    assert "offline_access" not in client["optionalClientScopes"] + client["defaultClientScopes"]
    assert client["optionalClientScopes"] == ["mcp:portal", "connector"]


@pytest.mark.parametrize("client_id", ["mcp-mobile", "mcp-local"])
def test_an_mcp_connector_token_names_only_the_resource_it_asked_for(realm_clients, rendered, client_id):
    """T-5611: the realm's default `roles` scope resolves an audience for every client whose role
    the person holds, so demo.viewer's mcp:portal token named `account` and 15 `app-*` clients
    beside portal-api. Without it (measured on Keycloak 26.8.0) the token's `aud` is `portal-api`
    alone and `realm_access.roles` stays, from the client's own mapper."""
    client = realm_clients[client_id]
    assert client["defaultClientScopes"] == ["acr", "basic", "email", "profile"]
    mappers = {m["name"]: m for m in client["protocolMappers"]}
    # amr names how the caller signed in (T-5567, on every platform client); no audience mapper.
    assert set(mappers) == {"groups", "realm-roles", "amr", "jc_org", "jc_groups"}, "no audience mapper on the client itself"
    assert not [m for m in mappers.values() if m["protocolMapper"] == "oidc-audience-mapper"]
    realm_roles = mappers["realm-roles"]
    assert realm_roles["protocolMapper"] == "oidc-usermodel-realm-role-mapper"
    assert realm_roles["config"]["claim.name"] == "realm_access.roles"
    assert realm_roles["config"]["access.token.claim"] == "true"
    for doc in rendered("local"):
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            realm = json.loads(base64.b64decode(doc["data"]["local.json"]))
    scope = {s["name"]: s for s in realm["clientScopes"]}["mcp:portal"]
    assert [m["config"]["included.client.audience"] for m in scope["protocolMappers"]] == ["portal-api"]


@pytest.fixture(scope="module")
def local_realm(rendered):
    return realm_of(rendered, "local")


def test_a_connector_token_names_one_connector_and_the_connectors_door_alone(local_realm, realm_clients):
    """T-5954, ADR-N-064 §3.3: Keycloak 26.8 mints no audience per connector, so the parameterized
    scope `connector:{id}` names the connector and its mapper names `portal-connectors`; the
    Portal refuses a token with a `connector:` scope on every route but /c/{id}. Measured on 26.8.1 with keycloak-config-cli 6.5.1-26 importing this
    block: claude.ai, Claude Code, VS Code and mcp-local tokens carry `aud: portal-connectors` and the
    one scope; `connector:../x` is refused invalid_scope."""
    scope = {s["name"]: s for s in local_realm["clientScopes"]}["connector"]
    attributes = scope["attributes"]
    assert attributes["is.parameterized.scope"] == "true"
    assert attributes["parameterized.scope.type"] == "custom"
    # The whole parameter is a connector id: 26 base32 characters, nothing else.
    assert attributes["parameterized.scope.regexp"] == "[a-z2-7]{26}"
    assert re.fullmatch(attributes["parameterized.scope.regexp"], "abcdefghijklmnopqrstuvwxyz")
    for bad in ["../x", "a" * 25, "a" * 27, "ABCDEFGHIJKLMNOPQRSTUVWXYZ", "*"]:
        assert not re.fullmatch(attributes["parameterized.scope.regexp"], bad)
    assert attributes["include.in.token.scope"] == "true"
    assert attributes["display.on.consent.screen"] == "true"
    mappers = {m["name"]: m for m in scope["protocolMappers"]}
    assert set(mappers) == {"portal-connectors-audience", "portal-api-audience", "groups", "amr", "jc_org", "jc_groups"}
    # portal-api exchanges the token for an Endpoint read: Keycloak's exchange refuses a subject
    # token whose `aud` lacks the requester ("Client is not within the token audience").
    assert mappers["portal-api-audience"]["config"]["included.client.audience"] == "portal-api"
    audience = mappers["portal-connectors-audience"]
    assert audience["protocolMapper"] == "oidc-audience-mapper"
    assert audience["config"] == {
        "included.custom.audience": "portal-connectors",
        "access.token.claim": "true",
        "id.token.claim": "false",
    }
    # The audience is the connectors' alone: no other scope or client names it.
    others = [
        m
        for owner in local_realm["clientScopes"] + list(realm_clients.values())
        if owner.get("name") != "connector"
        for m in owner.get("protocolMappers", [])
    ]
    assert not [m for m in others if "portal-connectors" in json.dumps(m)]
    # The two fixed connector clients may ask for it; a client the realm creates by CIMD gets it
    # and `offline_access` (claude.ai asks for a refresh token on every sign-in, T-6301) as its
    # only optional scopes.
    for client_id in ["mcp-mobile", "mcp-local"]:
        assert "connector" in realm_clients[client_id]["optionalClientScopes"]
    assert local_realm["defaultOptionalClientScopes"] == ["connector", "offline_access"]


def test_the_realm_optional_scopes_reach_no_managed_client(realm_clients):
    """T-6301: Keycloak links `defaultOptionalClientScopes` to a client created without its own
    list. Every keycloak-config-cli client names its list, so `offline_access` stays on the
    clients that chose it."""
    lacking = [c for c, client in realm_clients.items() if "optionalClientScopes" not in client]
    assert not lacking, lacking
    offline = {c for c, client in realm_clients.items() if "offline_access" in client["optionalClientScopes"]}
    assert offline == {"mcp-hub", "mcp-mobile"}, offline


def test_only_the_known_ai_clients_sign_in_by_their_client_id_url(local_realm):
    """T-5954: CIMD admits an https client id on claude.ai (claude.ai, Claude Code) or vscode.dev
    and fetches its document; its exact redirect URIs are the only ones it gets (measured: a
    foreign redirect is invalid_redirect_uri, a client id on another host client_not_found). No
    dynamic registration exists beside it."""
    (policy,) = local_realm["clientPolicies"]["policies"]
    assert policy["enabled"] is True and policy["profiles"] == ["cimd"]
    (condition,) = policy["conditions"]
    assert condition["condition"] == "client-id-uri"
    assert condition["configuration"] == {
        "client-id-uri-scheme": ["https"],
        "client-id-uri-allow-permitted-domains": ["claude.ai", "vscode.dev"],
    }
    (profile,) = local_realm["clientProfiles"]["profiles"]
    (executor,) = profile["executors"]
    assert executor["executor"] == "client-id-metadata-document"
    config = executor["configuration"]
    assert config["cimd-allow-permitted-domains"] == [
        "claude.ai", "vscode.dev", "code.visualstudio.com", "localhost", "127.0.0.1"
    ]
    assert not config.get("cimd-allow-http-scheme")
    assert not [d for d in config["cimd-allow-permitted-domains"] if "*" in d]
    # Resource indicators stay off: no static resource list stands in for the connector scope.
    assert "cimd-resource-indicator-allow-list" not in config


def test_mcp_hub_is_a_public_pkce_client_with_consent(realm_clients):
    """T-2490 / EP-88: the hub connector lives on a device too, so no secret and PKCE S256; its
    tokens name the hub, which the gateway accepts at /api/mcp only, and consent shows the
    person which `endpoint:{slug}` scopes a connector asks for (ADR-N-025 section 4)."""
    client = realm_clients["mcp-hub"]
    assert client["publicClient"] is True
    assert "secret" not in client
    assert client["consentRequired"] is True
    assert client["attributes"]["pkce.code.challenge.method"] == "S256"
    assert client["directAccessGrantsEnabled"] is False
    assert client["serviceAccountsEnabled"] is False
    audiences = [m["config"]["included.client.audience"] for m in client["protocolMappers"]
                 if m["protocolMapper"] == "oidc-audience-mapper"]
    assert audiences == ["mcp-hub"], "a hub token names the hub and nothing else"
    assert set(client["redirectUris"]) == set(realm_clients["mcp-mobile"]["redirectUris"])


def test_the_hub_s_one_login_scope_is_the_hub_s_alone_and_on_its_consent_screen(rendered, realm_clients):
    """T-4114, ADR-N-025 4b: `endpoint:*` reaches every Endpoint the person may read, so the
    person sees it before a token exists, and no other client's default token carries it."""
    for doc in rendered("local"):
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            realm = json.loads(base64.b64decode(doc["data"]["local.json"]))
    scope = {s["name"]: s for s in realm["clientScopes"]}["endpoint:*"]
    assert scope["attributes"]["display.on.consent.screen"] == "true"
    assert scope["attributes"]["include.in.token.scope"] == "true"
    assert scope["attributes"]["consent.screen.text"]
    assert not scope.get("protocolMappers"), "the scope adds no audience: the hub's own mapper names it"
    assert "endpoint:*" in realm_clients["mcp-hub"]["defaultClientScopes"]
    assert "roles" in realm_clients["mcp-hub"]["defaultClientScopes"], "the Endpoint Policies read the realm roles"
    for client_id, client in realm_clients.items():
        if client_id != "mcp-hub":
            assert "endpoint:*" not in client.get("defaultClientScopes", []), client_id
            assert "endpoint:*" not in client.get("optionalClientScopes", []), client_id
    assert "endpoint:*" not in realm.get("defaultDefaultClientScopes", [])


# --- PF-61, PF-79: who the caller is, on every client ----------------------------------------


def test_every_client_carries_the_groups_claim(realm_clients):
    """A `RoleBinding` names a group (`viewers` binds `platform-readers`), so a token without
    the claim makes every signed-in person a stranger: the Portal answers 404 on every list
    (PF-50, PF-61) and the forge lands nobody in its readers team (PF-79). Keycloak adds no
    such claim by default, and it broke silently on the clients nobody looked at."""
    for client_id, client in realm_clients.items():
        mappers = [m for m in client.get("protocolMappers", [])
                   if m["protocolMapper"] == "oidc-group-membership-mapper"]
        assert len(mappers) == 1, f"{client_id} carries {len(mappers)} group mappers"
        config = mappers[0]["config"]
        assert config["claim.name"] == "groups", client_id
        # ADR-N-067 2.2 (T-6408): a bare subgroup name `members` would match any Organization's.
        assert config["full.path"] == "true", f"{client_id}: a subgroup must arrive as its path"
        assert config["access.token.claim"] == "true" and config["id.token.claim"] == "true", client_id


def test_a_clients_own_mappers_survive_the_shared_one(realm_clients):
    """The shared mapper is added to a client's list, never over it: `mergeOverwrite` replaces
    a list wholesale, so writing the default the obvious way drops the audience mappers a
    token is refused without (T-0491, SDK-23)."""
    names = {m["name"] for m in realm_clients["portal-api"]["protocolMappers"]}
    assert {"groups", "realm-roles-in-id-token", "jc-functions-audience"} <= names


# --- T-1420: the forge's client alone skips PKCE until go-gitea/gitea#38202 ships ---------------


def test_the_gitea_client_drops_pkce_and_stays_confidential(realm_clients):
    gitea = realm_clients["gitea"]
    assert gitea["attributes"].get("pkce.code.challenge.method", "") == "", gitea["attributes"]
    assert gitea["attributes"]["post.logout.redirect.uris"] == "+"
    assert gitea["publicClient"] is False and gitea["secret"] == "$(CLIENT_SECRET_GITEA)"
    assert gitea["directAccessGrantsEnabled"] is False and gitea["implicitFlowEnabled"] is False
    uris = gitea["redirectUris"]
    assert all(u.endswith("/git/user/oauth2/keycloak/callback") and "*" not in u for u in uris), uris


def test_every_other_browser_client_still_requires_pkce(realm_clients):
    for client_id, client in realm_clients.items():
        if client_id == "gitea" or not client.get("standardFlowEnabled"):
            continue
        assert client["attributes"]["pkce.code.challenge.method"] == "S256", client_id


# --- T-2420, T-2271 (AG-52, PF-46): the agent proxy's two audiences ----------------------------


@pytest.fixture(scope="module")
def dev_realm_clients(rendered):
    """The realm of an environment that runs the agent runner; `local` leaves that component out."""
    for doc in rendered("dev"):
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            realm = json.loads(base64.b64decode(doc["data"]["dev.json"]))
            return {c["clientId"]: c for c in realm["clients"]}
    pytest.fail("no keycloak-config-cli realm Secret in the dev render")


def test_the_agent_proxy_carries_both_audiences_it_is_refused_without(dev_realm_clients):
    """One client, two doors, and a token is refused at either without its mapper.

    `context-gateway` is how a run reads samples through whichever endpoint it names (T-0666).
    `portal-internal` is how the proxy asks the Portal's internal listener what a run is
    (`authenticate_agent_proxy`, portal `src/auth/internal.rs`): without it the lookup fails,
    `RunResolver::fetch` reports no active run, and the proxy answers every model call of every
    conversation `401 invalid run credentials` — the wording is deliberately the same for four
    different failures (T-2285), so a dropped mapper looks exactly like a forged ticket. The
    live realm is checked by `scripts/smoke.sh`; this is the manifest it is written from.
    """
    proxy = dev_realm_clients["helsinki-agent-proxy"]
    audiences = {
        mapper["config"].get("included.custom.audience")
        for mapper in proxy["protocolMappers"]
        if mapper["protocolMapper"] == "oidc-audience-mapper"
    }
    assert {"context-gateway", "portal-internal"} <= audiences, sorted(a for a in audiences if a)
    # And it stays a workload: client credentials only, no browser flow to phish a token out of.
    assert proxy["serviceAccountsEnabled"] is True
    assert proxy["standardFlowEnabled"] is False and proxy["implicitFlowEnabled"] is False
    assert proxy["directAccessGrantsEnabled"] is False and proxy["publicClient"] is False


# --- T-2846 (PF-45, MF-14): a person's token for jcctl, by the device flow ---------------------


def test_jcctl_is_a_public_device_flow_client_for_the_portal(realm_clients):
    """`jcctl login` runs RFC 8628 on this client: it lives on a laptop, so no secret; no browser
    redirect lands anywhere, so no code flow and no redirect URI; and the token it hands out is
    for the Portal resource API, which accepts `aud: portal-api` and nothing else."""
    client = realm_clients["jcctl"]
    assert client["publicClient"] is True
    assert "secret" not in client, "a public client must not carry a secret"
    assert client["attributes"]["oauth2.device.authorization.grant.enabled"] == "true"
    assert client["standardFlowEnabled"] is False and client["implicitFlowEnabled"] is False
    assert client["directAccessGrantsEnabled"] is False and client["serviceAccountsEnabled"] is False
    assert client["redirectUris"] == [] and client["webOrigins"] == []
    audiences = {
        m["config"].get("included.client.audience") or m["config"].get("included.custom.audience")
        for m in client["protocolMappers"]
        if m["protocolMapper"] == "oidc-audience-mapper"
    }
    # T-5614: `jcctl-endpoint` reaches an Endpoint only beside the `endpoint:{slug}` scope of it.
    assert audiences == {"portal-api", "jcctl-endpoint"}, audiences
    # T-5614: a public client proves each sign-in with RFC 7636 S256; the realm refuses a device
    # authorization without it, and `jcctl login` sends it.
    assert client["attributes"]["pkce.code.challenge.method"] == "S256"
    # No default endpoint scope: a plain `jcctl login` token reaches no Endpoint.
    assert not any(s.startswith("endpoint:") for s in client["defaultClientScopes"]), client["defaultClientScopes"]
    assert not any(s.startswith("endpoint:") for s in client["optionalClientScopes"]), "the Job links them"


def test_only_jcctl_offers_the_device_flow(realm_clients):
    """A device code is a login a stranger can start and a person can be tricked into finishing
    (RFC 8628 §5.4), so the grant is on the one client that needs it."""
    offering = {
        client_id
        for client_id, client in realm_clients.items()
        if client.get("attributes", {}).get("oauth2.device.authorization.grant.enabled") == "true"
    }
    assert offering == {"jcctl"}


# --- T-2866, ADR-N-038 (AG-01, AG-94, AG-95): a run reads as its person, by token exchange -----


PROXY = "helsinki-agent-proxy"


def test_only_the_agent_proxy_the_gateway_and_the_portal_may_exchange_a_persons_token(dev_realm_clients):
    """Standard token exchange V2 on three clients: any other client that could turn a person's
    token into one of its own could read as that person (ADR-N-038 §3.2). The gateway exchanges
    only for a broker's audience on a read across brokers (T-3999,
    tests/test_broker_token_exchange.py); the Portal only for one Endpoint through the hub, on a
    connector's `jc_data_query` (T-6063, API/14 §4.2)."""
    exchanging = {
        client_id
        for client_id, client in dev_realm_clients.items()
        if client.get("attributes", {}).get("standard.token.exchange.enabled") == "true"
    }
    assert exchanging == {PROXY, "context-gateway", "portal-api"}
    attributes = dev_realm_clients[PROXY]["attributes"]
    # A refresh token for the run, tied to the person's own session: it ends when they sign out
    # (ADR-N-038 §3.5), which an offline token would not.
    assert attributes["standard.token.exchange.enableRefreshRequestedTokenType"] == "SAME_SESSION"


def test_only_the_portal_may_ask_for_the_hub_audience_in_an_exchange(rendered):
    """T-6063: the realm hands the Portal's exchanged token the `mcp-hub` audience only through
    `gateway-hub`, an optional scope of `portal-api` and of no other client, never a default
    one: no login token of any client names the hub through it. A connector's token names
    `portal-api` (the `connector` scope), so the Portal may exchange it."""
    for doc in rendered("dev"):
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            realm = json.loads(base64.b64decode(doc["data"]["dev.json"]))
            break
    else:
        pytest.fail("no keycloak-config-cli realm Secret in the dev render")
    scopes = {s["name"]: s for s in realm["clientScopes"]}
    hub = scopes["gateway-hub"]
    assert [
        (m["protocolMapper"], m["config"].get("included.client.audience")) for m in hub["protocolMappers"]
    ] == [("oidc-audience-mapper", "mcp-hub")]
    offering = {c["clientId"] for c in realm["clients"] if "gateway-hub" in c.get("optionalClientScopes", [])}
    assert offering == {"portal-api"}
    assert not [c["clientId"] for c in realm["clients"] if "gateway-hub" in c.get("defaultClientScopes", [])]
    assert "gateway-hub" not in realm.get("defaultDefaultClientScopes", [])
    assert "gateway-hub" not in realm.get("defaultOptionalClientScopes", [])
    connector = {
        m["config"].get("included.client.audience") for m in scopes["connector"]["protocolMappers"]
        if m["protocolMapper"] == "oidc-audience-mapper"
    }
    assert "portal-api" in connector


def test_a_persons_token_names_the_proxy_so_it_may_be_exchanged(dev_realm_clients):
    """V2 exchanges only a subject token whose audience names the requesting client: the edge's
    session token and the Portal's own login carry the proxy's client id."""
    for client_id in ("edge", "portal-api"):
        audiences = {
            m["config"].get("included.custom.audience") or m["config"].get("included.client.audience")
            for m in dev_realm_clients[client_id]["protocolMappers"]
            if m["protocolMapper"] == "oidc-audience-mapper"
        }
        assert PROXY in audiences, (client_id, audiences)


def test_the_proxys_account_declares_delegation_and_no_other_seed_account_does():
    """AG-95: the gateway decides an exchanged token as the person only for an account that says
    so; every other seeded workload keeps refusing a person's token."""
    import yaml
    from pathlib import Path

    declared = {}
    for path in Path(__file__).resolve().parent.parent.glob("components/*/seed/**/*.yaml"):
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == "ServiceAccount":
                name = f'{doc["metadata"].get("namespace")}/{doc["metadata"]["name"]}'
                declared[name] = (doc.get("spec") or {}).get("delegation")
    assert declared.get("helsinki/agent-proxy") == "token-exchange", declared
    assert {name for name, value in declared.items() if value} == {"helsinki/agent-proxy"}


@pytest.mark.parametrize("env", ["local", "production", "dev"])
def test_the_portal_writes_only_the_clients_it_manages(rendered, env):
    """T-2725, AP-27: the realm runs fine-grained admin permissions and, after every import, the
    admin-permissions Job grants `portal-api` the client writes and denies them on each client
    without `managed-by: joinedcontext`. The Job runs the Keycloak image's own kcadm.sh, holds no
    API token and reaches Keycloak through its own NetworkPolicy."""
    realm = realm_of(rendered, env)
    assert realm["adminPermissionsEnabled"] is True
    docs = rendered(env)
    job = next(d for d in docs if d.get("kind") == "Job" and d["metadata"]["name"] == "keycloak-admin-permissions")
    assert job["metadata"]["annotations"]["helm.sh/hook"] == "post-install,post-upgrade"
    pod = job["spec"]["template"]["spec"]
    assert pod["automountServiceAccountToken"] is False
    container = pod["containers"][0]
    assert container["image"].startswith("quay.io/keycloak/keycloak:") and "@sha256:" in container["image"]
    env_of = {e["name"]: e for e in container["env"]}
    assert env_of["PRINCIPAL"]["value"] == "portal-api"
    assert "secretKeyRef" in env_of["KEYCLOAK_PASSWORD"]["valueFrom"], "the password comes from the Secret, never a value"
    script = next(d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "keycloak-admin-permissions")
    assert "managed-by" in script["data"]["sync-admin-permissions.sh"]
    assert any(d.get("kind") == "NetworkPolicy" and "keycloak-admin-permissions" in d["metadata"]["name"] for d in docs)


@pytest.mark.parametrize("env", ["local", "production", "dev"])
def test_the_endpoint_scope_link_also_runs_every_five_minutes(rendered, env):
    """T-6388: the Portal creates an Endpoint's `endpoint:{slug}` scope at once and the deny keeps
    it from linking it, so a CronJob runs the hook's script on a clock: the pod the Job runs, the
    hook's ConfigMap, one run at a time, and a connector reads a new Endpoint minutes after Live."""
    docs = rendered(env)
    job = next(d for d in docs if d.get("kind") == "Job" and d["metadata"]["name"] == "keycloak-admin-permissions")
    cron = next(d for d in docs if d.get("kind") == "CronJob" and d["metadata"]["name"] == "keycloak-admin-permissions-sync")
    assert cron["spec"]["schedule"] == "*/5 * * * *"
    assert cron["spec"]["concurrencyPolicy"] == "Forbid"
    assert "annotations" not in cron["metadata"], "the CronJob is a release object, never a hook"
    assert cron["spec"]["jobTemplate"]["spec"]["template"] == job["spec"]["template"]
    volumes = cron["spec"]["jobTemplate"]["spec"]["template"]["spec"]["volumes"]
    assert {"name": "script", "configMap": {"name": "keycloak-admin-permissions"}} in volumes


# --- T-3311 (EP-103): the Portal asks the gateway's access/simulate with its own token ----------


def test_the_portal_carries_the_simulate_audience_and_never_the_gateways(realm_clients):
    """The gateway answers `access/simulate` for the audience `context-gateway-simulate` alone,
    and only from the Portal's own service account. The mapper puts that audience on the
    Portal's access tokens; `context-gateway` stays off them, or a person's Portal token would
    reach every Endpoint's data paths."""
    audiences = {
        mapper["config"].get("included.custom.audience")
        for mapper in realm_clients["portal-api"]["protocolMappers"]
        if mapper["protocolMapper"] == "oidc-audience-mapper"
    }
    assert "context-gateway-simulate" in audiences, sorted(a for a in audiences if a)
    assert "context-gateway" not in audiences
    simulate = next(
        m for m in realm_clients["portal-api"]["protocolMappers"]
        if m["config"].get("included.custom.audience") == "context-gateway-simulate"
    )
    assert simulate["config"]["access.token.claim"] == "true"
    assert simulate["config"]["id.token.claim"] == "false"
