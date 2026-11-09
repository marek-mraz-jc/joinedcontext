"""Smoke suite `kaisatsu`: sign-in on a rehearsal, against prod's sign-in documents as the reference.

   bins/jc/smoke/run kaisatsu --base-url <url> --export <bundle> --out <dir> [--state <jc state>]
       [--prod <fixtures/prod-identity-*.json>] [--keep]

Prod is never contacted: fetch_prod_identity.py captured its anonymous documents once (the realm's
OpenID configuration, the gateway's OAuth metadata, five authorize pages without signing in).

1. discovery: jc's realm configuration against prod's. Each endpoint, grant type, response type,
   PKCE method, client authentication method and signing algorithm prod advertises must be on jc
   when a client of the export's realm uses it (a client's flags and attributes say which); one
   no client uses is a pass naming that. Keycloak's own clients (admin console, account) count
   for nothing: kaisatsu has its own admin.
2. gateway: the gateway's anonymous OAuth server metadata, field by field (issuer host aside).
3. authorize: the same five authorize requests on jc: the same kind of answer (login form,
   error page, refusal text) as prod, except the CIMD connector client, which prod's realm does
   not know and jc's connectors need (`/c/{id}`, scope `connector:<id> offline_access`).
4. With --state (the realm admin client's and portal-api's secrets): a test person of the realm
   the state names signs in by code + PKCE (`mcp-local`), reads userinfo, refreshes, has a
   reused refresh token revoke its family, logs out in the browser; signs in to the Portal and
   out again; has its token exchanged by portal-api for one Endpoint (audience mcp-hub, scope
   endpoint:<slug>) and read there, while the same token on another Endpoint, its own unexchanged
   token and a forged one are refused. Refusals: a wrong password, another realm, the revoked
   family, the foreign audience. The connector door's metadata and 401, and the gateway's
   anonymous register, authorize, token and hub call close it.

No token, code, secret or password reaches results.json or REPORT.md (redact())."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import secrets
import sys
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent / "mcp"))
sys.path.append(str(HERE.parent / "endpoints"))

import common
from common import Http, Results, as_json, jwt_claims, loopback_only, read_token
from fetch_prod_identity import (
    CONNECTOR_ID,
    PROBES,
    authorize_query,
    page_kind,
)

FIXTURES = HERE / "fixtures"
ADMIN_CLIENT = "portal-reconciler"
EXCHANGE_CLIENT = "portal-api"
GATEWAY_CLIENT = "context-gateway"
BROKER_SCOPE = "context-broker-main"  # prod realm smartcity's broker scope
PKCE_CLIENT = "mcp-local"
LOOPBACK_CALLBACK = "http://localhost/callback"
HUB_AUDIENCE = "mcp-hub"

# Keycloak's own clients: the admin console, the account console and the internals they need.
# kaisatsu has its own admin, so what only they enable is no prod feature jc must carry.
BUILTIN_CLIENTS = {"admin-cli", "account", "account-console", "broker", "realm-management",
                   "security-admin-console", "admin-permissions"}

# Discovery endpoints and the realm feature a client must use for prod's to count; None: always.
ENDPOINTS = {
    "authorization_endpoint": None, "token_endpoint": None, "userinfo_endpoint": None,
    "end_session_endpoint": None, "jwks_uri": None, "introspection_endpoint": None,
    "revocation_endpoint": None, "pushed_authorization_request_endpoint": None,
    "device_authorization_endpoint": "grant:urn:ietf:params:oauth:grant-type:device_code",
    "backchannel_authentication_endpoint": "grant:urn:openid:params:grant-type:ciba",
    "check_session_iframe": "check-session-iframe",
    "registration_endpoint": "registration",
}
# kaisatsu answers the password grant and leaves it out of discovery on purpose (handlers.rs
# openid_configuration, T-5611): a client asking it gets it all the same.
HIDDEN_GRANTS = {"password"}
AUTHENTICATORS = {"client-secret": {"client_secret_basic", "client_secret_post"},
                  "client-jwt": {"private_key_jwt"}, "federated-jwt": {"private_key_jwt"},
                  "client-secret-jwt": {"client_secret_jwt"}, "client-x509": {"tls_client_auth"}}

SECRET_PARAMS = re.compile(
    r"((?:access_token|refresh_token|id_token|id_token_hint|code|client_secret|password|subject_token|"
    r"client_assertion|code_verifier)[\"']?\s*[=:]\s*[\"']?)[^&\"'\s,}]+", re.IGNORECASE)
JWT = re.compile(r"eyJ[\w-]+\.[\w-]+\.[\w-]*")


def redact(text) -> str:
    """Any text bound for results: tokens, codes, secrets and passwords become `<redacted>`."""
    s = JWT.sub("<jwt>", str(text))
    return SECRET_PARAMS.sub(lambda m: m.group(1) + "<redacted>", s)


def realm_of(state) -> str:
    # jc-migrate puts realm_of in common.py; until that lands the same rule reads the file here.
    if hasattr(common, "realm_of"):
        return common.realm_of(state)
    p = Path(state) / "realm-name.txt" if state else None
    name = p.read_text().strip() if p and p.is_file() else ""
    return name or "jc"


# ------------------------------------------------------------------------- what prod's realm uses

def realm_file(export: Path, realm: str) -> Path | None:
    kc = export / "keycloak"
    f = kc / f"realm-{realm}.json"
    if f.is_file():
        return f
    rest = sorted(p for p in kc.glob("realm-*.json") if p.name != "realm-master.json")
    return rest[0] if rest else None


def used_features(realm: dict, export_files: Path | None = None) -> dict:
    """{feature: [client ids]} the realm's own clients (Keycloak's aside) use."""
    used: dict[str, list] = {}

    def mark(feature, cid):
        used.setdefault(feature, []).append(cid)

    for c in realm.get("clients") or []:
        cid = c.get("clientId", "")
        if cid in BUILTIN_CLIENTS or c.get("enabled") is False:
            continue
        at = c.get("attributes") or {}

        def on(k, at=at):
            return str(at.get(k, "")).lower() == "true"

        if c.get("standardFlowEnabled"):
            mark("grant:authorization_code", cid)
            mark("grant:refresh_token", cid)
            mark("response:code", cid)
        if c.get("implicitFlowEnabled"):
            mark("grant:implicit", cid)
            mark("response:implicit", cid)
        if c.get("directAccessGrantsEnabled"):
            mark("grant:password", cid)
        if c.get("serviceAccountsEnabled"):
            mark("grant:client_credentials", cid)
        if on("oauth2.device.authorization.grant.enabled"):
            mark("grant:urn:ietf:params:oauth:grant-type:device_code", cid)
        if on("oidc.ciba.grant.enabled"):
            mark("grant:urn:openid:params:grant-type:ciba", cid)
        if on("standard.token.exchange.enabled"):
            mark("grant:urn:ietf:params:oauth:grant-type:token-exchange", cid)
        if on("oauth2.jwt.authorization.grant.enabled"):
            mark("grant:urn:ietf:params:oauth:grant-type:jwt-bearer", cid)
        if c.get("authorizationServicesEnabled"):
            mark("grant:urn:ietf:params:oauth:grant-type:uma-ticket", cid)
        if at.get("pkce.code.challenge.method"):
            mark(f"pkce:{at['pkce.code.challenge.method']}", cid)
        if not c.get("publicClient") and not c.get("bearerOnly"):
            for m in AUTHENTICATORS.get(c.get("clientAuthenticatorType") or "client-secret", ()):
                mark(f"auth:{m}", cid)
        alg = at.get("id.token.signed.response.alg")
        if alg:
            mark(f"alg:{alg}", cid)
    if realm.get("defaultSignatureAlgorithm"):
        mark(f"alg:{realm['defaultSignatureAlgorithm']}", "realm default")
    if export_files and export_files.is_dir():
        for f in export_files.rglob("*"):
            if f.suffix in (".js", ".ts", ".tsx", ".jsx", ".html", ".json") and f.is_file() and f.stat().st_size < 2_000_000:
                text = f.read_text(errors="ignore")
                if "keycloak-js" in text or "login-status-iframe" in text:
                    mark("check-session-iframe", str(f.relative_to(export_files)))
    return used


def rel(url, issuer) -> str | None:
    if not isinstance(url, str) or not issuer:
        return None
    return url.removeprefix(issuer)


def who(used: dict, feature: str) -> str:
    ids = used.get(feature) or []
    return f"{len(ids)} prod client(s), e.g. {', '.join(ids[:3])}" if ids else ""


def compare_discovery(prod: dict, jc: dict, used: dict) -> list[tuple]:
    """Rows (id, expected, actual, ok, reason) of jc's OpenID configuration against prod's."""
    rows = []
    pi, ji = prod.get("issuer", ""), jc.get("issuer", "")
    shape = re.compile(r"/realms/[^/]+$")
    rows.append(("issuer", "an issuer ending in /realms/<realm>, as prod's", redact(ji),
                 bool(ji) and bool(shape.search(ji)) and bool(shape.search(pi)), ""))
    for key, feature in ENDPOINTS.items():
        if key not in prod:
            continue
        want, got = rel(prod[key], pi), rel(jc.get(key), ji)
        if feature is None or used.get(feature):
            rows.append((f"endpoint.{key}", f"{want} (prod's path)", got or "absent", got == want,
                         who(used, feature) if feature else ""))
        else:
            rows.append((f"endpoint.{key}", f"{want} only if a prod client uses it", got or "absent", True,
                         f"no prod client uses {feature}"))
    jc_grants = set(jc.get("grant_types_supported") or []) | HIDDEN_GRANTS
    for g in prod.get("grant_types_supported") or []:
        feature = f"grant:{g}"
        if used.get(feature):
            note = " (handled, left out of discovery: handlers.rs, T-5611)" if g in HIDDEN_GRANTS else ""
            rows.append((f"grant.{g}", "supported (a prod client uses it)",
                         ("supported" + note) if g in jc_grants else "absent", g in jc_grants, who(used, feature)))
        else:
            rows.append((f"grant.{g}", "supported only if a prod client uses it",
                         "supported" if g in jc_grants else "absent", True, f"no prod client enables {g}"))
    jc_resp = set(jc.get("response_types_supported") or [])
    for rt in prod.get("response_types_supported") or []:
        feature = "response:code" if rt == "code" else ("response:implicit" if "token" in rt else f"response:{rt}")
        need = bool(used.get(feature))
        rows.append((f"response_type.{rt}", "supported" if need else "supported only if a prod client uses it",
                     "supported" if rt in jc_resp else "absent", rt in jc_resp or not need,
                     who(used, feature) or f"no prod client uses response_type {rt}"))
    jc_pkce = set(jc.get("code_challenge_methods_supported") or [])
    for m in prod.get("code_challenge_methods_supported") or []:
        need = m == "S256" or bool(used.get(f"pkce:{m}"))
        rows.append((f"pkce.{m}", "supported" if need else "supported only if a prod client pins it",
                     "supported" if m in jc_pkce else "absent", m in jc_pkce or not need,
                     who(used, f"pkce:{m}") or ("" if need else f"no prod client pins PKCE {m}")))
    jc_auth = set(jc.get("token_endpoint_auth_methods_supported") or [])
    for m in prod.get("token_endpoint_auth_methods_supported") or []:
        need = bool(used.get(f"auth:{m}"))
        rows.append((f"client_auth.{m}", "supported" if need else "supported only if a prod client uses it",
                     "supported" if m in jc_auth else "absent", m in jc_auth or not need,
                     who(used, f"auth:{m}") or f"no prod client authenticates by {m}"))
    jc_alg = set(jc.get("id_token_signing_alg_values_supported") or [])
    for feature in sorted(f for f in used if f.startswith("alg:")):
        alg = feature[4:]
        rows.append((f"id_token_alg.{alg}", "supported (prod signs with it)", "supported" if alg in jc_alg else "absent",
                     alg in jc_alg, who(used, feature)))
    return rows


def compare_gateway(prod: dict, jc: dict) -> list[tuple]:
    """The gateway's OAuth metadata, field by field, each value with its issuer swapped for jc's."""
    pi, ji = prod.get("issuer", ""), jc.get("issuer", "")

    def norm(v):
        if isinstance(v, str):
            return v.replace(pi, "<issuer>", 1) if pi and v.startswith(pi) else v
        if isinstance(v, list):
            return sorted(norm(x) for x in v)
        return v

    def jnorm(v):
        if isinstance(v, str):
            return v.replace(ji, "<issuer>", 1) if ji and v.startswith(ji) else v
        if isinstance(v, list):
            return sorted(jnorm(x) for x in v)
        return v

    rows = []
    for k in sorted(prod):
        want, got = norm(prod[k]), jnorm(jc.get(k)) if k in jc else "absent"
        rows.append((f"field.{k}", json.dumps(want), json.dumps(got) if k in jc else "absent", want == got, ""))
    for k in sorted(set(jc) - set(prod)):
        rows.append((f"field.{k}", "absent (prod has none)", json.dumps(jnorm(jc[k])), True, "jc adds it"))
    return rows


def same_answer(prod: dict, jc: dict) -> bool:
    if prod.get("page") != jc.get("page"):
        return False
    if prod.get("page") == "refusal":
        return prod.get("message") == jc.get("message")
    return (prod.get("status", 0) // 100) == (jc.get("status", 0) // 100)


def brief(p: dict) -> str:
    return redact(f"{p.get('status')} {p.get('page')} {p.get('message', '')}".strip())


# ------------------------------------------------------------------------------- live rehearsal

def forged_jwt() -> str:
    """A JWT shape no realm signed: header {"alg": "RS256"}, empty claims, a made-up signature."""
    part = lambda d: base64.urlsafe_b64encode(json.dumps(d).encode()).rstrip(b"=").decode()
    return f"{part({'alg': 'RS256'})}.{part({})}.{secrets.token_urlsafe(16)}"


def has_form(page: bytes) -> bool:
    return b'name="password"' in page


def form_of(page: bytes) -> str:
    return "login form" if has_form(page) else "no form"


def pkce_pair() -> tuple[str, str]:
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    return verifier, challenge


class Live:
    """The sign-in paths on the rehearsal, as one test person of its realm."""

    def __init__(self, a, res: Results, realm: str):
        from personas import Browser, _follow, _login
        self.Browser, self.follow, self.login_form = Browser, _follow, _login
        self.a, self.r, self.realm = a, res, realm
        self.base = a.base_url.rstrip("/")
        self.http = Http(self.base)
        self.state = Path(a.state)
        self.kc = f"/id/realms/{realm}/protocol/openid-connect"
        self.user = f"smoke-kaisatsu-{secrets.token_hex(3)}"
        self.password = secrets.token_urlsafe(18) + "-7aZ"
        self.uid = ""

    def secret(self, name: str) -> str:
        return (self.state / name).read_text().strip()

    def form(self, path: str, fields: dict, token: str | None = None):
        st, _h, raw = self.http.call("POST", path, token=token, body=urllib.parse.urlencode(fields).encode(),
                                    headers={"Content-Type": "application/x-www-form-urlencoded"})
        return st, as_json(raw), raw

    def admin(self) -> str:
        st, body, _ = self.form(f"{self.kc}/token", {"grant_type": "client_credentials", "client_id": ADMIN_CLIENT,
                                                    "client_secret": self.secret("portal-admin-client-secret")})
        if st != 200:
            raise RuntimeError(f"{ADMIN_CLIENT} client credentials: {st}")
        return body["access_token"]

    def make_person(self) -> None:
        admin = self.admin()
        st, _, raw = self.http.call("POST", f"/id/admin/realms/{self.realm}/users", token=admin, body={
            "username": self.user, "enabled": True, "email": f"{self.user}@rehearsal.invalid",
            "emailVerified": True, "firstName": "Smoke", "lastName": "Kaisatsu"})
        if st not in (201, 409):
            raise RuntimeError(f"create {self.user}: {st} {redact(raw[:160])}")
        st, _, raw = self.http.call("GET", f"/id/admin/realms/{self.realm}/users?username={self.user}&exact=true",
                                    token=admin)
        self.uid = json.loads(raw)[0]["id"]
        st, _, _ = self.http.call("PUT", f"/id/admin/realms/{self.realm}/users/{self.uid}/reset-password",
                                  token=admin, body={"type": "password", "value": self.password, "temporary": False})
        if st not in (200, 204):
            raise RuntimeError(f"password of {self.user}: {st}")

    def remove_person(self) -> str:
        if not self.uid:
            return ""
        st, _, _ = self.http.call("DELETE", f"/id/admin/realms/{self.realm}/users/{self.uid}", token=self.admin())
        return "" if st in (200, 204) else f"delete {self.user}: {st}"

    def authorize(self, b, client: str, redirect: str, scope: str, extra: dict | None = None):
        """(page url, status, body, verifier, nonce) of an authorize request in browser `b`."""
        verifier, challenge = pkce_pair()
        nonce = secrets.token_urlsafe(12)
        q = {"client_id": client, "redirect_uri": redirect, "response_type": "code", "scope": scope,
             "state": "smoke-state", "nonce": nonce, "code_challenge": challenge, "code_challenge_method": "S256",
             **(extra or {})}
        url, st, page = self.follow(b, f"{self.kc}/auth?{urllib.parse.urlencode(q)}", stop=redirect)
        return url, st, page, verifier, nonce

    def code_flow(self, b, scope: str = "openid mcp:portal") -> tuple[dict, str]:
        """Signs the person in at `mcp-local`; answers (token answer, nonce sent)."""
        url, st, page, verifier, nonce = self.authorize(b, PKCE_CLIENT, LOOPBACK_CALLBACK, scope)
        if st != 200:
            raise RuntimeError(f"authorize {PKCE_CLIENT}: {st}")
        loc = self.login_form(b, url, page, self.user, self.password)
        if not loc.startswith(LOOPBACK_CALLBACK):
            loc, _, _ = self.follow(b, urllib.parse.urljoin(url, loc), stop=LOOPBACK_CALLBACK)
        q = urllib.parse.parse_qs(urllib.parse.urlparse(loc).query)
        if q.get("state") != ["smoke-state"]:
            raise RuntimeError("the callback did not carry the state back")
        st, body, raw = self.form(f"{self.kc}/token", {
            "grant_type": "authorization_code", "client_id": PKCE_CLIENT, "code": q.get("code", [""])[0],
            "redirect_uri": LOOPBACK_CALLBACK, "code_verifier": verifier})
        if st != 200 or not isinstance(body, dict):
            raise RuntimeError(f"code for token: {st} {redact(raw[:160])}")
        return body, nonce

    def row(self, id, target, call, expected, actual, ok, reason=""):
        self.r.check(id, target, redact(call), expected, redact(actual), ok, redact(reason))

    # ---------------------------------------------------------------------------- the checks

    def run(self, endpoints: list[str], other_realm: str | None) -> None:
        self.make_person()
        b = self.Browser(self.base)
        try:
            tok, nonce = self.code_flow(b)
        except Exception as err:  # noqa: BLE001 - a broken sign-in is a failing row, never a pass
            self.row("signin.code-pkce", PKCE_CLIENT, "authorize + login + code for token", "tokens", str(err)[:200], False)
            return
        have = sorted(k for k in ("access_token", "refresh_token", "id_token") if tok.get(k))
        self.row("signin.code-pkce", PKCE_CLIENT, f"{self.kc}/auth (code + S256) -> login -> token",
                 "access, refresh and id token", ", ".join(have), len(have) == 3)
        at, idt = jwt_claims(tok.get("access_token")), jwt_claims(tok.get("id_token"))
        issuer = f"{self.base}/id/realms/{self.realm}"
        self.row("signin.claims", PKCE_CLIENT, "the access and id token's claims",
                 f"iss {issuer}, sub the person, azp {PKCE_CLIENT}, id token nonce echoed",
                 f"iss {at.get('iss')}, sub {'the person' if at.get('sub') == self.uid else at.get('sub')}, "
                 f"azp {at.get('azp')}, nonce {'echoed' if idt.get('nonce') == nonce else 'wrong'}",
                 at.get("iss") == issuer and at.get("sub") == self.uid and at.get("azp") == PKCE_CLIENT
                 and idt.get("nonce") == nonce)
        st, _h, raw = self.http.call("GET", f"{self.kc}/userinfo", token=tok["access_token"])
        info = as_json(raw) or {}
        self.row("signin.userinfo", PKCE_CLIENT, f"GET {self.kc}/userinfo", "200, sub the person",
                 f"{st}, sub {'the person' if info.get('sub') == self.uid else info.get('sub')}",
                 st == 200 and info.get("sub") == self.uid)
        self.exchange(tok["access_token"], endpoints)
        self.other_realm(tok, other_realm)
        self.refresh_family(tok["refresh_token"])
        self.browser_logout()
        self.wrong_password()
        self.portal()
        self.connector_door()
        self.anonymous_gateway()

    def refresh_family(self, r0: str) -> None:
        st, body, _ = self.form(f"{self.kc}/token", {"grant_type": "refresh_token", "client_id": PKCE_CLIENT,
                                                    "refresh_token": r0})
        r1 = (body or {}).get("refresh_token") if st == 200 else None
        self.row("refresh.rotate", PKCE_CLIENT, "refresh_token grant", "200, a new access and a new refresh token",
                 f"{st}, refresh {'rotated' if r1 and r1 != r0 else 'not rotated'}", st == 200 and bool(r1) and r1 != r0,
                 "the realm revokes refresh tokens (revokeRefreshToken true, max reuse 0), as prod's")
        st2, body2, _ = self.form(f"{self.kc}/token", {"grant_type": "refresh_token", "client_id": PKCE_CLIENT,
                                                      "refresh_token": r0})
        self.row("refresh.reuse-refused", PKCE_CLIENT, "the spent refresh token again", "400 invalid_grant",
                 f"{st2} {(body2 or {}).get('error')}", st2 == 400 and (body2 or {}).get("error") == "invalid_grant")
        if not r1:
            self.r.skip("refresh.family-revoked", PKCE_CLIENT, "the rotated refresh token after the reuse",
                        "400 invalid_grant", "no rotated refresh token to try")
            return
        st3, body3, _ = self.form(f"{self.kc}/token", {"grant_type": "refresh_token", "client_id": PKCE_CLIENT,
                                                      "refresh_token": r1})
        self.row("refresh.family-revoked", PKCE_CLIENT, "the rotated refresh token after the reuse",
                 "400 invalid_grant (the reuse revoked the family)", f"{st3} {(body3 or {}).get('error')}",
                 st3 == 400 and (body3 or {}).get("error") == "invalid_grant")

    def browser_logout(self) -> None:
        b = self.Browser(self.base)
        try:
            tok, _ = self.code_flow(b)
        except Exception as err:  # noqa: BLE001
            self.row("logout.browser", PKCE_CLIENT, "a second sign-in", "tokens", str(err)[:200], False)
            return
        # Signed in: a new authorize in the same browser answers a code without the form.
        _, st, page, _, _ = self.authorize(b, PKCE_CLIENT, LOOPBACK_CALLBACK, "openid mcp:portal")
        self.row("logout.sso-before", PKCE_CLIENT, "authorize again while signed in",
                 "no login form (the session signs it in)", f"{st}, {form_of(page)}",
                 not has_form(page))
        q = urllib.parse.urlencode({"id_token_hint": tok["id_token"], "post_logout_redirect_uri": LOOPBACK_CALLBACK,
                                    "client_id": PKCE_CLIENT})
        url, st, _ = self.follow(b, f"{self.kc}/logout?{q}", stop=LOOPBACK_CALLBACK)
        self.row("logout.end-session", PKCE_CLIENT, f"GET {self.kc}/logout (id_token_hint, post_logout_redirect_uri)",
                 "back at the post-logout address", f"{st} at {urllib.parse.urlparse(url).netloc}{urllib.parse.urlparse(url).path}",
                 url.startswith(LOOPBACK_CALLBACK))
        _, st, page, _, _ = self.authorize(b, PKCE_CLIENT, LOOPBACK_CALLBACK, "openid mcp:portal")
        self.row("logout.sso-after", PKCE_CLIENT, "authorize again after the logout", "200, the login form",
                 f"{st}, {form_of(page)}",
                 st == 200 and has_form(page))
        st, body, _ = self.form(f"{self.kc}/token", {"grant_type": "refresh_token", "client_id": PKCE_CLIENT,
                                                    "refresh_token": tok["refresh_token"]})
        self.row("logout.refresh-refused", PKCE_CLIENT, "the session's refresh token after the logout",
                 "400 invalid_grant", f"{st} {(body or {}).get('error')}", st == 400)

    def wrong_password(self) -> None:
        b = self.Browser(self.base)
        url, _st, page, _, _ = self.authorize(b, PKCE_CLIENT, LOOPBACK_CALLBACK, "openid")
        try:
            loc = self.login_form(b, url, page, self.user, self.password + "-wrong")
            self.row("refuse.wrong-password", PKCE_CLIENT, "the login form with a wrong password",
                     "no code (the form again)", f"redirected to {urllib.parse.urlparse(loc).path} with a code", False)
        except RuntimeError as err:
            self.row("refuse.wrong-password", PKCE_CLIENT, "the login form with a wrong password",
                     "no code (the form again)", str(err)[:120], "code=" not in str(err))

    def other_realm(self, tok: dict, other: str | None) -> None:
        if not other:
            for id in ("refuse.other-realm.userinfo", "refuse.other-realm.login"):
                self.r.skip(id, "realm", "another realm of this jc", "refused",
                            "the rehearsal's kaisatsu serves one realm: no other realm to try")
            return
        okc = f"/id/realms/{other}/protocol/openid-connect"
        st, _, _ = self.http.call("GET", f"{okc}/userinfo", token=tok["access_token"])
        self.row("refuse.other-realm.userinfo", other, f"GET {okc}/userinfo with a {self.realm} token", "401", str(st), st == 401)
        b = self.Browser(self.base)
        _verifier, challenge = pkce_pair()
        q = urllib.parse.urlencode({"client_id": PKCE_CLIENT, "redirect_uri": LOOPBACK_CALLBACK, "response_type": "code",
                                    "scope": "openid", "state": "s", "code_challenge": challenge,
                                    "code_challenge_method": "S256"})
        url, st, page = self.follow(b, f"{okc}/auth?{q}", stop=LOOPBACK_CALLBACK)
        if st != 200 or not has_form(page):
            self.row("refuse.other-realm.login", other, f"{okc}/auth for {PKCE_CLIENT}", "no code for this person",
                     f"{st}, no login form ({PKCE_CLIENT} unknown there)", st in (400, 404))
            return
        try:
            self.login_form(b, url, page, self.user, self.password)
            ok, detail = False, "a code for a person of another realm"
        except RuntimeError as err:
            ok, detail = "code=" not in str(err), str(err)[:120]
        self.row("refuse.other-realm.login", other, f"{okc}/auth + the {self.realm} person's password",
                 "no code (the person is not in that realm)", detail, ok)

    def exchange(self, subject: str, endpoints: list[str]) -> None:
        """portal-api exchanges the person's token for one Endpoint (auth/oidc.rs exchange_for_endpoint);
        the gateway (auth/subject.rs admit_on) takes it there and refuses it elsewhere."""
        if len(endpoints) < 2:
            self.r.skip("exchange", "gateway", "token exchange", "two public NGSI-LD Endpoints",
                        "the export holds fewer than two public Endpoints serving NGSI-LD")
            return
        own, other = endpoints[0], endpoints[1]
        st, body, _raw = self.form(f"{self.kc}/token", {
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange", "client_id": EXCHANGE_CLIENT,
            "client_secret": self.secret("portal-client-secret"), "subject_token": subject,
            "subject_token_type": "urn:ietf:params:oauth:token-type:access_token",
            "requested_token_type": "urn:ietf:params:oauth:token-type:access_token",
            "audience": HUB_AUDIENCE, "scope": f"gateway-hub endpoint:{own}"})
        tok = (body or {}).get("access_token") if st == 200 else None
        claims = jwt_claims(tok)
        aud = claims.get("aud")
        aud = aud if isinstance(aud, list) else [aud]
        self.row("exchange.issued", EXCHANGE_CLIENT, f"token exchange, audience {HUB_AUDIENCE}, scope endpoint:{own}",
                 f"200, aud {HUB_AUDIENCE}, azp {EXCHANGE_CLIENT}, scope endpoint:{own}",
                 f"{st} {(body or {}).get('error', '')} {(body or {}).get('error_description', '')}".strip()
                 + (f", aud {aud}, azp {claims.get('azp')}, scope {claims.get('scope')}" if tok else ""),
                 bool(tok) and HUB_AUDIENCE in aud and claims.get("azp") == EXCHANGE_CLIENT
                 and f"endpoint:{own}" in str(claims.get("scope", "")).split())
        read = "/api/endpoint/{}/ngsi-ld/v1/types"
        # The exchange prod's realm enables (context-gateway: standard.token.exchange.enabled): a
        # federated read's broker token (federation/exchange.rs). admit_on takes that audience only
        # inside a federated read, so on a plain Endpoint it is a foreign audience.
        gst, gbody, _ = self.form(f"{self.kc}/token", {
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange", "client_id": GATEWAY_CLIENT,
            "client_secret": self.secret("gateway-client-secret"), "subject_token": subject,
            "subject_token_type": "urn:ietf:params:oauth:token-type:access_token",
            "requested_token_type": "urn:ietf:params:oauth:token-type:access_token", "scope": BROKER_SCOPE})
        gtok = (gbody or {}).get("access_token") if gst == 200 else None
        gc = jwt_claims(gtok)
        # Keycloak 26's standard exchange (and kaisatsu's) refuses a subject token whose aud lacks
        # the requester (knowledge/deployment.md, T-5954): the person's mcp-local token decides.
        saud = jwt_claims(subject).get("aud")
        named = GATEWAY_CLIENT in (saud if isinstance(saud, list) else [saud])
        got = (f"{gst} {(gbody or {}).get('error', '')} {(gbody or {}).get('error_description', '')}".strip()
               + (f", azp {gc.get('azp')}, sub {'the person' if gc.get('sub') == self.uid else gc.get('sub')}" if gtok else ""))
        if named:
            self.row("exchange.gateway.issued", GATEWAY_CLIENT, f"token exchange by {GATEWAY_CLIENT}, scope {BROKER_SCOPE}",
                     f"200, azp {GATEWAY_CLIENT}, the person's sub", got,
                     bool(gtok) and gc.get("azp") == GATEWAY_CLIENT and gc.get("sub") == self.uid)
        else:
            self.row("refuse.exchange-not-in-audience", GATEWAY_CLIENT,
                     f"token exchange by {GATEWAY_CLIENT} of a token whose aud {saud} lacks it",
                     "400 (the requester is not in the subject token's audience)", got, gst == 400 and not gtok)
        if gtok:
            gs, _, _ = self.http.call("GET", read.format(own), token=gtok)
            self.row("refuse.broker-audience", own, f"GET {read.format(own)} with the broker token, outside a federated read",
                     "401 (a foreign audience here)", str(gs), gs == 401)
        st0, _, _ = self.http.call("GET", read.format(other))
        self.row("exchange.control-anonymous", other, f"GET {read.format(other)} without a token",
                 "200 (a public Endpoint)", str(st0), st0 == 200)
        st1, _, _ = self.http.call("GET", read.format(own), token=forged_jwt())
        self.row("exchange.control-forged", own, f"GET {read.format(own)} with a forged token", "401", str(st1), st1 == 401)
        if not tok:
            for id in ("exchange.read", "refuse.foreign-audience", "refuse.unexchanged"):
                self.r.skip(id, "gateway", "a read with the exchanged token", "", "the realm refused the exchange")
            return
        st2, _, raw2 = self.http.call("GET", read.format(own), token=tok)
        self.row("exchange.read", own, f"GET {read.format(own)} with the exchanged token", "200 (admit_on takes the aud)",
                 f"{st2} {raw2[:80].decode(errors='replace')}", st2 == 200)
        st3, _, _ = self.http.call("GET", read.format(other), token=tok)
        self.row("refuse.foreign-audience", other, f"GET {read.format(other)} with the token exchanged for {own}",
                 "401 (another Endpoint's token, though the Endpoint is public)", str(st3), st3 == 401)
        st4, _, _ = self.http.call("GET", read.format(own), token=subject)
        self.row("refuse.unexchanged", own, f"GET {read.format(own)} with the person's mcp:portal token",
                 "401 (no Endpoint audience)", str(st4), st4 == 401)

    def portal(self) -> None:
        b = self.Browser(self.base)
        url, st, page = self.follow(b, "/api/v1/auth/login")
        if st != 200 or not has_form(page):
            self.row("portal.signin", "portal", "GET /api/v1/auth/login", "the realm's login form", f"{st}", False)
            return
        try:
            loc = self.login_form(b, url, page, self.user, self.password)
            _, st, _ = self.follow(b, urllib.parse.urljoin(url, loc))
        except RuntimeError as err:
            self.row("portal.signin", "portal", "/api/v1/auth/login -> login -> callback", "a session", str(err)[:160], False)
            return
        st, _, raw = b.call("GET", "/api/v1/auth/me")
        me = as_json(raw) or {}
        ident = me.get("identity") or me  # Me flattens the identity (auth/oidc.rs me)
        self.row("portal.signin", "portal", "/api/v1/auth/login -> login -> callback, GET /api/v1/auth/me",
                 "200, the person", f"{st}, {'the person' if ident.get('subject') == self.uid else ident.get('subject')}",
                 st == 200 and ident.get("subject") == self.uid)
        csrf = b.cookies.get("__Host-jc_csrf", "")
        st, _, raw = b.call("POST", "/api/v1/auth/logout", headers={"X-CSRF-Token": csrf}, body={})
        target = (as_json(raw) or {}).get("endSessionUrl", "")
        self.row("portal.logout", "portal", "POST /api/v1/auth/logout", "200 naming the realm's end-session address",
                 f"{st}, {urllib.parse.urlparse(target).path}", st == 200 and target.split("?")[0].endswith("/logout"))
        if target:
            self.follow(b, target, stop=self.base + "/")
        st, _, _ = b.call("GET", "/api/v1/auth/me")
        _, st2, page2 = self.follow(b, "/api/v1/auth/login")
        self.row("portal.logout.after", "portal", "GET /api/v1/auth/me, then /api/v1/auth/login",
                 "401, then the login form (the realm session ended too)",
                 f"{st}, then {st2} {form_of(page2)}",
                 st == 401 and has_form(page2))

    def connector_door(self) -> None:
        cid = CONNECTOR_ID
        st, _, raw = self.http.call("GET", f"/c/{cid}/.well-known/oauth-protected-resource")
        doc = as_json(raw) or {}
        issuer = f"{self.base}/id/realms/{self.realm}"
        self.row("connector.metadata", f"/c/{cid}", f"GET /c/{cid}/.well-known/oauth-protected-resource",
                 f"200, authorization server {issuer}, scope connector:{cid}",
                 f"{st}, {doc.get('authorization_servers')}, {doc.get('scopes_supported')}",
                 st == 200 and issuer in (doc.get("authorization_servers") or [])
                 and doc.get("scopes_supported") == [f"connector:{cid}"])
        st, h, _ = self.http.call("POST", f"/c/{cid}", body={"jsonrpc": "2.0", "id": 1, "method": "initialize"})
        www = h.get("www-authenticate", "")
        self.row("connector.challenge", f"/c/{cid}", f"POST /c/{cid} without a token",
                 f"401 naming the metadata and scope connector:{cid}", f"{st} {www[:160]}",
                 st == 401 and "resource_metadata=" in www and f"connector:{cid}" in www)

    def anonymous_gateway(self) -> None:
        """ADR-N-052: the gateway's stateless server. Register, authorize, token, then the hub."""
        st, _, raw = self.http.call("POST", "/oauth/register", body={"redirect_uris": [LOOPBACK_CALLBACK],
                                                                     "client_name": "jc smoke kaisatsu"})
        client = (as_json(raw) or {}).get("client_id")
        self.row("anonymous.register", "gateway", "POST /oauth/register", "201, a client_id", str(st), st == 201 and bool(client))
        if not client:
            return
        verifier, challenge = pkce_pair()
        q = urllib.parse.urlencode({"client_id": client, "redirect_uri": LOOPBACK_CALLBACK, "response_type": "code",
                                    "state": "s", "code_challenge": challenge, "code_challenge_method": "S256"})
        st, h, _ = self.http.call("GET", f"/oauth/authorize?{q}")
        loc = h.get("location", "")
        code = urllib.parse.parse_qs(urllib.parse.urlparse(loc).query).get("code", [""])[0]
        self.row("anonymous.authorize", "gateway", "GET /oauth/authorize", "302 to the callback with a code",
                 f"{st} to {urllib.parse.urlparse(loc).netloc}{urllib.parse.urlparse(loc).path}",
                 st == 302 and loc.startswith(LOOPBACK_CALLBACK) and bool(code))
        st, body, _ = self.form("/oauth/token", {"grant_type": "authorization_code", "client_id": client, "code": code,
                                                 "redirect_uri": LOOPBACK_CALLBACK, "code_verifier": verifier})
        tok = (body or {}).get("access_token") if st == 200 else None
        self.row("anonymous.token", "gateway", "POST /oauth/token (code + verifier)", "200, an access token",
                 str(st), bool(tok))
        if tok:
            st, _, raw = self.http.call("POST", "/api/mcp", token=tok, headers={"Accept": "application/json, text/event-stream"},
                                        body={"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                                            "protocolVersion": "2025-06-18", "capabilities": {},
                                            "clientInfo": {"name": "jc-smoke-kaisatsu", "version": "1"}}})
            self.row("anonymous.hub", "/api/mcp", "POST /api/mcp initialize with that token", "200 (worth a call without one)",
                     str(st), st == 200)


# ------------------------------------------------------------------------------------- the suite

def public_ngsi_endpoints(export: Path) -> list[str]:
    import export as ex
    eps = ex.load_endpoints(export / "gitea" / "files")
    return sorted(e.slug for e in eps if e.audience == "public" and not e.closed and "ngsi-ld" in e.representations)


def latest_fixture() -> Path | None:
    found = sorted(FIXTURES.glob("prod-identity-*.json"))
    return found[-1] if found else None


def run(a, res: Results) -> dict:
    prod = json.loads(Path(a.prod).read_text())
    realm = realm_of(a.state)
    export = Path(a.export)
    rf = realm_file(export, realm)
    realm_doc = json.loads(rf.read_text()) if rf else {}
    used = used_features(realm_doc, export / "gitea" / "files")
    http = Http(a.base_url)
    base = a.base_url.rstrip("/")

    path = f"/id/realms/{realm}/.well-known/openid-configuration"
    st, _, raw = http.call("GET", path)
    jc_doc = as_json(raw) if st == 200 else None
    res.check("discovery.document", "kaisatsu", f"GET {path}", "200, JSON", str(st), isinstance(jc_doc, dict))
    if isinstance(jc_doc, dict):
        if not rf:
            res.skip("discovery.usage", "kaisatsu", "the export's realm file", "the realm prod's clients live in",
                     f"no keycloak/realm-*.json in {export}: every prod field counts as used")
        for id, want, got, ok, why in compare_discovery(prod["openidConfiguration"], jc_doc, used):
            res.check(f"discovery.{id}", "kaisatsu", f"GET {path}", want, got, ok, why)

    st, _, raw = http.call("GET", "/.well-known/oauth-authorization-server")
    gw = as_json(raw) if st == 200 else None
    res.check("gateway.document", "gateway", "GET /.well-known/oauth-authorization-server", "200, JSON", str(st),
              isinstance(gw, dict))
    if isinstance(gw, dict):
        for id, want, got, ok, why in compare_gateway(prod["gatewayMetadata"], gw):
            res.check(f"gateway.{id}", "gateway", "GET /.well-known/oauth-authorization-server", want, got, ok, why)

    from personas import Browser, _follow
    for pid, (door, client, redirect, scope) in PROBES.items():
        url = (f"/id/realms/{realm}/protocol/openid-connect/auth" if door == "kc" else "/oauth/authorize")
        q = authorize_query(client, redirect or base + "/", scope)
        # A browser follows the realm's own redirects (kaisatsu answers 303 to its login page,
        # Keycloak the page at once); a redirect to the client's address is the answer itself.
        at, st, raw = _follow(Browser(base), f"{url}?{q}", stop=(redirect or base + "/") + "?")
        got = page_kind(st, {"location": at} if st in (301, 302, 303, 307, 308) else {}, raw)
        want = prod["authorize"][pid]
        call = f"GET {url} client_id={client}" + (f" scope={scope}" if scope else "")
        if pid == "cimd-claude":
            res.check("authorize.cimd-claude.prod", "prod realm", "fixture: prod's answer to the same request",
                      "recorded", brief(want), True,
                      "prod's realm has no CIMD: claude.ai connects through the gateway's DCR and mcp-hub")
            res.check(f"authorize.{pid}", "kaisatsu", call, "200, the login form (the connector sign-in jc offers)",
                      brief(got), got.get("page") == "login",
                      "jc's /c/{id} connectors need it (connector:<id> offline_access, CIMD client)")
            continue
        res.check(f"authorize.{pid}", "kaisatsu" if door == "kc" else "gateway", call,
                  f"as prod: {brief(want)}", brief(got), same_answer(want, got))

    if not a.state:
        res.skip("live", "rehearsal", "a test person's sign-in, refresh, logout, exchange", "the paths run",
                 "no --state: the realm admin client's and portal-api's secrets live there")
        return {"realm": realm, "prod": Path(a.prod).name}
    others = []
    for name in ("jc", "master"):
        if name != realm and http.call("GET", f"/id/realms/{name}")[0] == 200:
            others.append(name)
    live = Live(a, res, realm)
    try:
        live.run(public_ngsi_endpoints(export), others[0] if others else None)
    except Exception as err:  # noqa: BLE001 - a broken fixture is a failing row, never a pass
        res.check("live.setup", "rehearsal", "test person and sign-in", "set up", redact(str(err))[:240], False)
    finally:
        if not a.keep:
            left = live.remove_person()
            if left:
                res.check("cleanup", "rehearsal", "remove the smoke-kaisatsu person", "removed", left, False)
    return {"realm": realm, "prod": Path(a.prod).name, "person": live.user}


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="run kaisatsu", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--base-url", required=True)
    p.add_argument("--export", required=True, help="the export bundle the rehearsal was built from")
    p.add_argument("--out", required=True)
    p.add_argument("--state", help="the rehearsal's state dir ($REHEARSAL_HOME/<env>/state)")
    p.add_argument("--prod", default=str(latest_fixture() or ""), help="prod-identity-*.json (default: the newest)")
    p.add_argument("--keep", action="store_true", help="leave the smoke-kaisatsu person")
    p.add_argument("--admin-token-file", help="unused (accepted for the common runner)")
    p.add_argument("--person-token-file", help="unused: the suite makes its own person")
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    read_token(a.admin_token_file)
    if not a.prod:
        p.error("no prod-identity-*.json: run fetch_prod_identity.py once")
    res = Results("kaisatsu", a.base_url)
    meta = run(a, res)
    res.write(Path(a.out), meta)
    t = res.totals()
    print(f"kaisatsu: {t['pass']} pass, {t['fail']} fail, {t['skip']} skip -> {a.out}/REPORT.md")
    return 1 if t["fail"] else 0


if __name__ == "__main__":
    sys.exit(main())
