"""Selftest of the kaisatsu suite over a fixture: the committed prod capture against a stub jc on
loopback (kaisatsu's discovery document, the gateway's metadata, the authorize answers), and a stub
that answers 200 to everything, which must fail.

    python3 -m pytest bins/jc/smoke/kaisatsu/tests -q
"""

import importlib.util
import json
import sys
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

HERE = Path(__file__).resolve().parent
SUITE_DIR = HERE.parent
sys.path.insert(0, str(SUITE_DIR))
sys.path.insert(0, str(SUITE_DIR.parent))

_spec = importlib.util.spec_from_file_location("smoke_kaisatsu_suite", SUITE_DIR / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)

PROD = json.loads(max((SUITE_DIR / "fixtures").glob("prod-identity-*.json")).read_text())
REALM = "smartcity"


def realm_doc():
    return {"realm": REALM, "defaultSignatureAlgorithm": "RS256", "clients": [
        {"clientId": "admin-cli", "publicClient": True, "directAccessGrantsEnabled": True},
        {"clientId": "apisix-gateway", "directAccessGrantsEnabled": True, "clientAuthenticatorType": "client-secret"},
        {"clientId": "jcctl", "publicClient": True, "attributes": {"oauth2.device.authorization.grant.enabled": "true"}},
        {"clientId": "mcp-local", "publicClient": True, "standardFlowEnabled": True,
         "attributes": {"pkce.code.challenge.method": "S256"}},
        {"clientId": "portal-api", "standardFlowEnabled": True, "serviceAccountsEnabled": True,
         "clientAuthenticatorType": "client-secret"},
        {"clientId": "context-gateway", "serviceAccountsEnabled": True, "clientAuthenticatorType": "federated-jwt",
         "attributes": {"standard.token.exchange.enabled": "true"}},
        {"clientId": "admin-permissions", "authorizationServicesEnabled": True},
    ]}


def kaisatsu_doc(issuer, device=False):
    """The shape of kaisatsu's handlers.rs openid_configuration."""
    b = f"{issuer}/protocol/openid-connect"
    d = {"issuer": issuer, "authorization_endpoint": f"{b}/auth", "token_endpoint": f"{b}/token",
         "introspection_endpoint": f"{b}/token/introspect", "userinfo_endpoint": f"{b}/userinfo",
         "end_session_endpoint": f"{b}/logout", "revocation_endpoint": f"{b}/revoke",
         "pushed_authorization_request_endpoint": f"{b}/ext/par/request", "jwks_uri": f"{b}/certs",
         "response_types_supported": ["code"],
         "grant_types_supported": ["authorization_code", "refresh_token", "client_credentials",
                                   "urn:ietf:params:oauth:grant-type:token-exchange",
                                   "urn:ietf:params:oauth:grant-type:jwt-bearer"],
         "id_token_signing_alg_values_supported": ["ES256", "RS256", "EdDSA"],
         "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post", "private_key_jwt"],
         "code_challenge_methods_supported": ["S256"]}
    if device:
        d["device_authorization_endpoint"] = f"{b}/auth/device"
        d["grant_types_supported"].append("urn:ietf:params:oauth:grant-type:device_code")
    return d


LOGIN = b'<form><input name="csrf_token" value="x"><input name="username"><input name="password"></form>'


def stub(handler_answer):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            st, ctype, body = handler_answer(self)
            self.send_response(st)
            self.send_header("Content-Type", ctype)
            self.end_headers()
            self.wfile.write(body)

        do_POST = do_GET

    srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


CIMD_POLICY = {"policies": [{"name": "cimd", "enabled": True, "profiles": ["cimd"], "conditions": [
    {"condition": "client-id-uri", "configuration": {"client-id-uri-allow-permitted-domains": ["claude.ai"]}}]}]}


def jc_like(device=False, cimd=False):
    """kaisatsu's answers; `cimd`: the realm has the client-id-uri policy, so a URL client_id signs in."""
    def answer(h):
        base = f"http://127.0.0.1:{h.server.server_port}"
        u = urllib.parse.urlparse(h.path)
        if u.path == f"/id/realms/{REALM}/.well-known/openid-configuration":
            return 200, "application/json", json.dumps(kaisatsu_doc(f"{base}/id/realms/{REALM}", device)).encode()
        if u.path == "/.well-known/oauth-authorization-server":
            meta = {k: (v.replace(PROD["gatewayIssuer"], base) if isinstance(v, str) else v)
                    for k, v in PROD["gatewayMetadata"].items()}
            return 200, "application/json", json.dumps(meta).encode()
        if u.path == "/oauth/authorize":
            return 400, "text/plain", b"The request was refused: the client_id is not one this server issued, or it expired; register again.\n"
        if u.path.endswith("/protocol/openid-connect/auth"):
            client = urllib.parse.parse_qs(u.query).get("client_id", [""])[0]
            if client.startswith("https://") and not cimd:
                return 401, "application/json", b'{"error":"invalid_client","error_description":"unknown client_id"}'
            return 200, "text/html", LOGIN
        return 404, "text/plain", b"no"
    return answer


def export_dir(tmp_path, realm=None):
    kc = tmp_path / "export" / "keycloak"
    kc.mkdir(parents=True)
    (kc / f"realm-{REALM}.json").write_text(json.dumps(realm or realm_doc()))
    (tmp_path / "export" / "gitea" / "files").mkdir(parents=True)
    state = tmp_path / "state"
    state.mkdir()
    (state / "realm-name.txt").write_text(REALM + "\n")
    return tmp_path / "export", state


def run_suite(tmp_path, answer, monkeypatch, realm=None):
    monkeypatch.setattr(suite, "realm_of", lambda s: REALM)
    srv = stub(answer)
    try:
        export, _state = export_dir(tmp_path, realm)
        out = tmp_path / "out"
        # Without --state the live part is one named skip.
        a = suite.argparse.Namespace(base_url=f"http://127.0.0.1:{srv.server_port}", export=str(export), out=str(out), state=None, keep=False,
                                     prod=str(max((SUITE_DIR / "fixtures").glob("prod-identity-*.json"))))
        res = suite.Results("kaisatsu", a.base_url)
        suite.run(a, res)
        res.write(out)
        return {r["id"]: r for r in json.loads((out / "results.json").read_text())["checks"]}
    finally:
        srv.shutdown()


def test_a_used_prod_feature_jc_lacks_fails_and_an_unused_one_passes():
    used = suite.used_features(realm_doc())
    assert "jcctl" in used["grant:urn:ietf:params:oauth:grant-type:device_code"]
    assert "admin-cli" not in used.get("grant:password", [])  # Keycloak's own client counts for nothing
    assert "grant:urn:ietf:params:oauth:grant-type:uma-ticket" not in used  # only admin-permissions has it
    rows = {r[0]: r for r in suite.compare_discovery(PROD["openidConfiguration"],
                                                      kaisatsu_doc("http://l/id/realms/smartcity"), used)}
    assert rows["endpoint.device_authorization_endpoint"][3] is False
    assert rows["grant.urn:ietf:params:oauth:grant-type:device_code"][3] is False
    assert rows["grant.password"][3] is True and "T-5611" in rows["grant.password"][2]
    assert rows["grant.implicit"][3] is True and "no prod client" in rows["grant.implicit"][4]
    assert rows["endpoint.backchannel_authentication_endpoint"][3] is True
    assert rows["endpoint.token_endpoint"][3] is True
    assert rows["client_auth.private_key_jwt"][3] is True and rows["client_auth.client_secret_basic"][3] is True
    assert rows["id_token_alg.RS256"][3] is True
    fixed = {r[0]: r for r in suite.compare_discovery(PROD["openidConfiguration"],
                                                       kaisatsu_doc("http://l/id/realms/smartcity", device=True), used)}
    assert fixed["endpoint.device_authorization_endpoint"][3] is True


def test_gateway_metadata_compares_paths_not_hosts():
    prod = PROD["gatewayMetadata"]
    jc = {k: (v.replace(PROD["gatewayIssuer"], "http://127.0.0.1:9") if isinstance(v, str) else v) for k, v in prod.items()}
    assert all(r[3] for r in suite.compare_gateway(prod, jc))
    jc.pop("registration_endpoint")
    jc["token_endpoint"] = "http://127.0.0.1:9/token"
    bad = {r[0] for r in suite.compare_gateway(prod, jc) if not r[3]}
    assert bad == {"field.registration_endpoint", "field.token_endpoint"}


def test_redact_keeps_no_token_code_secret_or_password():
    text = ('{"access_token":"eyJabc.def.ghi","refresh_token":"r1"} http://localhost/callback?code=XYZ&state=s '
            'client_secret=s3cr3t password=hunter2 id_token_hint=eyJa.b.c')
    out = suite.redact(text)
    for leak in ("eyJabc", "r1\"", "XYZ", "s3cr3t", "hunter2", "eyJa.b"):
        assert leak not in out, leak
    assert "state=s" in out


def test_the_suite_on_a_jc_like_stub_fails_only_the_missing_device_grant(tmp_path, monkeypatch):
    rows = run_suite(tmp_path, jc_like(), monkeypatch)
    fails = sorted(i for i, r in rows.items() if r["result"] == "fail")
    assert fails == ["discovery.endpoint.device_authorization_endpoint",
                     "discovery.grant.urn:ietf:params:oauth:grant-type:device_code"]
    assert rows["authorize.portal-ui"]["result"] == "pass"
    assert rows["authorize.gateway-unknown-client"]["result"] == "pass"
    assert rows["live"]["result"] == "skip" and "--state" in rows["live"]["reason"]
    rows = run_suite(tmp_path / "fixed", jc_like(device=True), monkeypatch)
    assert not [i for i, r in rows.items() if r["result"] == "fail" and i.startswith("discovery.")]


def test_a_url_client_id_signs_in_only_where_the_realm_has_the_cimd_policy(tmp_path, monkeypatch):
    # Prod's realm (no client-id-uri policy): refused as prod refuses it; a login form fails.
    rows = run_suite(tmp_path / "prod", jc_like(), monkeypatch)
    assert rows["authorize.cimd-claude"]["result"] == "pass", rows["authorize.cimd-claude"]
    assert "owner decision" in rows["authorize.cimd-claude"]["reason"]
    rows = run_suite(tmp_path / "prod-login", jc_like(cimd=True), monkeypatch)
    assert rows["authorize.cimd-claude"]["result"] == "fail"
    # A realm with the policy (jc's shipped realm): the connector sign-in; a refusal fails.
    with_policy = {**realm_doc(), "clientPolicies": CIMD_POLICY}
    rows = run_suite(tmp_path / "jc", jc_like(cimd=True), monkeypatch, with_policy)
    assert rows["authorize.cimd-claude"]["result"] == "pass"
    rows = run_suite(tmp_path / "jc-refused", jc_like(), monkeypatch, with_policy)
    assert rows["authorize.cimd-claude"]["result"] == "fail"


def test_the_shipped_realm_allows_cimd_and_portal_api_exchange_and_prod_neither():
    shipped = json.loads(suite.SHIPPED_REALM.read_text())
    assert suite.cimd_allowed(shipped) and suite.exchanges(shipped, "portal-api")
    prod = realm_doc()
    assert not suite.cimd_allowed(prod) and not suite.exchanges(prod, "portal-api")
    assert suite.exchanges(prod, "context-gateway")
    off = {"policies": [{**CIMD_POLICY["policies"][0], "enabled": False}]}
    assert not suite.cimd_allowed({"clientPolicies": off})


def test_the_served_realm_is_the_one_the_rehearsal_imports(tmp_path):
    export, state = export_dir(tmp_path)
    assert not suite.exchanges(suite.served_realm(export, REALM, str(state)), "portal-api")
    # The kit applied deployment main's portal-api lines (T-6063): that file decides.
    main = realm_doc()
    next(c for c in main["clients"] if c["clientId"] == "portal-api")["attributes"] = {
        "standard.token.exchange.enabled": "true"}
    (state / "realms").mkdir()
    (state / "realms" / f"{REALM}.json").write_text(json.dumps(main))
    assert suite.exchanges(suite.served_realm(export, REALM, str(state)), "portal-api")
    assert suite.exchanges(suite.served_realm(tmp_path / "none", "jc"), "portal-api")  # jc's shipped realm


def test_a_stub_answering_200_to_everything_fails(tmp_path, monkeypatch):
    rows = run_suite(tmp_path, lambda h: (200, "application/json", b"{}"), monkeypatch)
    fails = {i for i, r in rows.items() if r["result"] == "fail"}
    assert "authorize.cimd-claude" in fails and "authorize.gateway-unknown-client" in fails
    assert any(i.startswith("gateway.field.") for i in fails)
    assert "discovery.issuer" in fails


def test_no_secret_reaches_the_results(tmp_path, monkeypatch):
    rows = run_suite(tmp_path, jc_like(), monkeypatch)
    text = json.dumps(rows)
    assert "eyJ" not in text and "code=" not in text
