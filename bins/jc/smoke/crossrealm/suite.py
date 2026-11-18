"""Smoke suite `crossrealm` (T-7230): each Organization signs in through one realm, its own, and a
token, session or code of that realm opens nothing on another Organization's hosts.

    bins/jc/smoke/run crossrealm --jc-config <the deployment's jc.toml> --out <dir>
        [--realm-admin-dir <dir>] [--organizations a,b]

The Organizations are the jc.toml's [[organizations]] with a sign-in host of their own (hosts.idm,
realm named like the Organization). Fewer than two: every row is one skip, "the deployment serves
one realm" (the rehearsal). Requests go to the real hosts over verified TLS, never followed by
redirect on their own; `Guard` refuses any host outside the Organizations' and the installation's
sign-in host and any write outside the list below, and <out>/requests.log names each request by
method, host, path and status alone (no query, header or body).

Anonymous rows, every Organization O and every other one P:
- realm.own.O: idm.O serves realm O, its issuer https://idm.O/realms/O (200).
- realm.foreign.O.P: idm.O answers 404 for realm P, for master and for the installation realm.
- realm.pinned.O: the installation's sign-in host answers 404 for realm O.
- one-realm.signin.O: the Portal's sign-in on O's host goes to realm O's authorize endpoint.
- one-realm.portal-mcp.O, one-realm.api.O, one-realm.mcp.O: the protected-resource metadata of the
  Portal's MCP door and of the gateway's hub names realm O and no other realm.
- one-realm.opendata.O: the open data sign-in hands over to O's own Portal host.
- one-realm.apps.O: skipped while jc.toml names no Apps domain of O.
- cookie.host-only.O: no cookie the Portal or the realm sets on O's sign-in names a Domain.

With --realm-admin-dir (a file per Organization, named like it, holding the secret of that realm's
`portal-reconciler` client; read from the secrets mount, never logged), for the first two
Organizations A and B of it, each way round:
- the run makes in realm A a smoke client (client credentials) and a test person (random password
  and a second factor this process holds), and deletes both at the end, also when a row fails;
- token.A.on.B.<host>: the client's token is accepted on A's Portal API, api, mcp and open data
  hosts (no 401) and refused 401 on B's;
- code.A.at.B: the person's authorization code of realm A is refused at B's token endpoint, and so
  is A's client with its secret;
- session.A.on.B: the Portal session of A's host opens nothing on B's host (401), and B's sign-in
  shows B's own form instead of a code for A's person.
"""
from __future__ import annotations

import argparse
import re
import secrets
import socket
import sys
import tomllib
import urllib.error
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3] / "deploy" / "rehearsal"))
import rehearsal as R  # noqa: E402  (cookie-jar client, redirects, the sign-in form)
sys.path.insert(0, str(HERE.parent))
from common import SECOND_FACTOR, Results, as_json  # noqa: E402

SUITE = "crossrealm"
ONE_REALM = "the deployment serves one realm: no second Organization realm to refuse"
ADMIN_CLIENT = "portal-reconciler"
PORTAL_CLIENT = "portal-api"
NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,62}")


def organizations(doc: dict) -> dict[str, dict]:
    """{name: hosts} of each [[organizations]] entry with its own sign-in host, hosts lowercased."""
    out = {}
    for org in doc.get("organizations") or []:
        hosts = {k: v.lower() for k, v in (org.get("hosts") or {}).items() if isinstance(v, str)}
        if NAME.fullmatch(org.get("name") or "") and hosts.get("idm") and hosts.get("portal"):
            out[org["name"]] = hosts
    return out


def routed(hosts: dict, resolve=socket.getaddrinfo) -> str | None:
    """None when every host of an Organization has a DNS answer, else the first that has none: an
    Organization the edge does not route yet (jc.toml lists it before its DNS records exist)."""
    for host in hosts.values():
        try:
            resolve(host.split(":")[0], 443)
        except OSError:
            return host
    return None


def installation_login(doc: dict) -> str:
    """The installation realm's sign-in base: kaisatsu's own host in host mode, /login on the base
    host otherwise."""
    scheme = doc.get("scheme", "https")
    mount = (doc.get("mounts") or {}).get("kaisatsu")
    host = mount.get("host") if isinstance(mount, dict) else None
    return f"{scheme}://{host}" if host else f"{scheme}://{doc['base_host']}/login"


def realms_named(doc) -> list[str]:
    """The realm issuers (".../realms/<name>") a protected-resource document names."""
    servers = doc.get("authorization_servers") if isinstance(doc, dict) else None
    return [s.rstrip("/") for s in servers or [] if isinstance(s, str) and "/realms/" in s]


class Refused(RuntimeError):
    pass


class Guard:
    """What the run may send: GET and HEAD to the hosts it knows; POST to a realm's token endpoint,
    its sign-in forms and an MCP initialize; the admin calls that make and remove the run's own
    client and person in a realm it holds the admin secret of."""

    WRITES = (
        ("POST", re.compile(r"/realms/(?P<r>[a-z0-9-]+)/protocol/openid-connect/token")),
        ("POST", re.compile(r"/realms/(?P<r>[a-z0-9-]+)/login-actions/[a-z-]+")),
        ("POST", re.compile(r"/api/mcp")),
        ("POST", re.compile(r"/admin/realms/(?P<r>[a-z0-9-]+)/(users|clients)")),
        ("PUT", re.compile(r"/admin/realms/(?P<r>[a-z0-9-]+)/users/[0-9a-zA-Z-]+/reset-password")),
        ("DELETE", re.compile(r"/admin/realms/(?P<r>[a-z0-9-]+)/(users|clients)/[0-9a-zA-Z-]+"
                              r"(/credentials/[0-9a-zA-Z-]+)?")),
    )

    def __init__(self, hosts: set[str], admin_realms: set[str], log: Path | None):
        self.hosts, self.admin_realms, self.log = hosts, admin_realms, log

    def check(self, method: str, url: str) -> None:
        u = urllib.parse.urlsplit(url)
        if u.scheme != "https" or (u.hostname or "") not in self.hosts:
            raise Refused(f"{method} {u.scheme}://{u.hostname}: not a host of this run")
        if method in ("GET", "HEAD"):
            return
        for m, pattern in self.WRITES:
            hit = pattern.fullmatch(u.path) if m == method else None
            if hit and (not u.path.startswith("/admin/") or hit.group("r") in self.admin_realms):
                return
        raise Refused(f"{method} {u.hostname}{u.path}: outside the run's write list")

    def note(self, method: str, url: str, status: int) -> None:
        if self.log:
            u = urllib.parse.urlsplit(url)
            with self.log.open("a") as f:
                f.write(f"{method} {u.hostname}{u.path} {status}\n")


class Http(R.Http):
    """The rehearsal client (one cookie jar for every host: a browser's worst case for isolation),
    each call checked by the guard first and logged without query, headers or body."""

    def __init__(self, guard: Guard, base: str = ""):
        super().__init__(base)
        self.guard = guard
        self.set_cookies: list[str] = []  # the last answer's Set-Cookie headers

    def _store(self, headers):
        self.set_cookies = list(headers.get_all("Set-Cookie") or [])
        super()._store(headers)

    def call(self, method, path, body=None, headers=None, form=None, timeout=30):
        """(status, headers with lower-case names, body); status 0 when the host does not answer."""
        self.guard.check(method, self.url(path))
        self.set_cookies = []
        try:
            st, hdrs, raw = super().call(method, path, body=body, headers=headers, form=form, timeout=timeout)
        except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
            st, hdrs, raw = 0, {}, str(e).encode()
        self.guard.note(method, self.url(path), st)
        return st, {k.lower(): v for k, v in hdrs.items()}, raw


class Suite:
    def __init__(self, doc: dict, res: Results, guard: Guard, admin_dir: Path | None, pick: list[str]):
        self.doc, self.r, self.guard = doc, res, guard
        self.scheme = doc.get("scheme", "https")
        self.orgs = organizations(doc)
        self.login = installation_login(doc)
        self.installation = (doc.get("identity") or {}).get("realm") or "jc"
        self.admin_dir, self.pick = admin_dir, pick

    def anon(self) -> Http:
        return Http(self.guard)

    def url(self, org: str, role: str, path: str) -> str:
        return f"{self.scheme}://{self.orgs[org][role]}{path}"

    def issuer(self, org: str) -> str:
        return self.url(org, "idm", f"/realms/{org}")

    # ------------------------------------------------------------------ anonymous rows
    def realms(self) -> None:
        h = self.anon()
        for o in self.orgs:
            st, _, raw = h.call("GET", self.url(o, "idm", f"/realms/{o}/.well-known/openid-configuration"))
            iss = (as_json(raw) or {}).get("issuer") if st == 200 else None
            self.r.check(f"realm.own.{o}", self.orgs[o]["idm"], f"GET /realms/{o}/.well-known/openid-configuration",
                         f"200, issuer {self.issuer(o)}", f"{st} {iss or ''}".strip(), iss == self.issuer(o))
            for p in [*(x for x in self.orgs if x != o), "master", self.installation]:
                st, _, _ = h.call("GET", self.url(o, "idm", f"/realms/{p}/.well-known/openid-configuration"))
                self.r.check(f"realm.foreign.{o}.{p}", self.orgs[o]["idm"],
                             f"GET /realms/{p}/.well-known/openid-configuration", "404", str(st), st == 404)
            st, _, _ = h.call("GET", f"{self.login}/realms/{o}/.well-known/openid-configuration")
            self.r.check(f"realm.pinned.{o}", urllib.parse.urlsplit(self.login).hostname,
                         f"GET /realms/{o}/.well-known/openid-configuration", "404 (realm on its own host alone)",
                         str(st), st == 404)

    def metadata(self, o: str, role: str, path: str, row: str) -> None:
        host = self.orgs[o].get(role)
        if not host:
            self.r.skip(row, role, f"GET {path}", f"realm {o} alone", f"jc.toml names no {role} host of {o}")
            return
        st, _, raw = self.anon().call("GET", f"{self.scheme}://{host}{path}")
        named = realms_named(as_json(raw))
        self.r.check(row, host, f"GET {path}", f"200 naming {self.issuer(o)} alone",
                     f"{st} {', '.join(named) or 'no realm'}", st == 200 and named == [self.issuer(o)])

    def one_realm(self) -> None:
        for o, hosts in self.orgs.items():
            h = self.anon()
            st, hdrs, _ = h.call("GET", self.url(o, "portal", "/api/v1/auth/login"))
            loc = hdrs.get("location") or ""
            want = f"{self.issuer(o)}/protocol/openid-connect/auth"
            self.r.check(f"one-realm.signin.{o}", hosts["portal"], "GET /api/v1/auth/login",
                         f"303 to {want}", f"{st} {loc.split('?')[0]}", st in (302, 303) and loc.split("?")[0] == want)
            self.cookies(o, h)
            self.metadata(o, "portal", "/.well-known/oauth-protected-resource/api/v1/mcp", f"one-realm.portal-mcp.{o}")
            self.metadata(o, "api", "/.well-known/oauth-protected-resource/api/mcp", f"one-realm.api.{o}")
            if hosts.get("mcp", hosts.get("api")) != hosts.get("api"):
                self.metadata(o, "mcp", "/.well-known/oauth-protected-resource/api/mcp", f"one-realm.mcp.{o}")
            if hosts.get("opendata"):
                st, hdrs, _ = h.call("GET", self.url(o, "opendata", "/user/login"))
                to = urllib.parse.urlsplit(hdrs.get("location") or "").hostname
                self.r.check(f"one-realm.opendata.{o}", hosts["opendata"], "GET /user/login",
                             f"a redirect to {hosts['portal']} (its sign-in)", f"{st} {to or ''}".strip(),
                             st in (302, 303, 307) and to == hosts["portal"])
            if hosts.get("apps"):
                self.r.skip(f"one-realm.apps.{o}", hosts["apps"], "an App's sign-in", f"realm {o}",
                            "no App of this Organization is named to the run")
            else:
                self.r.skip(f"one-realm.apps.{o}", "apps", "an App's sign-in", f"realm {o}",
                            f"jc.toml names no Apps domain of {o}")

    def cookies(self, o: str, h: Http) -> None:
        """Follows O's sign-in to its form and reads every Set-Cookie on the way: each host-only."""
        url, seen, wide = self.url(o, "portal", "/api/v1/auth/login"), 0, []
        for _ in range(6):
            st, hdrs, _ = h.call("GET", url)
            for c in h.set_cookies:
                seen += 1
                if re.search(r";\s*domain\s*=", c, re.IGNORECASE):
                    wide.append(f"{urllib.parse.urlsplit(url).hostname}: {c.split('=', 1)[0]}")
            loc = hdrs.get("location")
            if st not in (301, 302, 303, 307, 308) or not loc:
                break
            url = urllib.parse.urljoin(url, loc)
        self.r.check(f"cookie.host-only.{o}", self.orgs[o]["portal"], "the sign-in's Set-Cookie headers",
                     "no Domain attribute", f"{seen} cookies, {', '.join(wide) or 'none with Domain'}",
                     seen > 0 and not wide)

    # ------------------------------------------------------------------ rows with a realm-A token
    def credentialed(self) -> None:
        held = [o for o in (self.pick or self.orgs) if o in self.orgs and self.admin_dir
                and (self.admin_dir / o).is_file()]
        rows = ("token", "code", "session")
        if len(held) < 2:
            why = ("no --realm-admin-dir: the portal-reconciler secret of two Organizations' realms, "
                   "from the secrets mount" if not self.admin_dir else
                   f"--realm-admin-dir holds the secret of {len(held)} Organization(s), two are needed")
            for row in rows:
                self.r.skip(f"{row}.cross-realm", "realm", f"a {row} of one realm on another's hosts", "refused", why)
            return
        a, b = held[:2]
        for own, other in ((a, b), (b, a)):
            made = Made(self, own)
            try:
                made.make()
                self.tokens(own, other, made)
                self.codes(own, other, made)
                self.sessions(own, other, made)
            except (RuntimeError, KeyError, TypeError) as e:
                self.r.check(f"setup.{own}", self.orgs[own]["idm"], "the smoke client and person of the realm",
                             "made", str(e)[:200], False)
            finally:
                for row, ok, detail in made.remove():
                    self.r.check(f"cleanup.{own}.{row}", self.orgs[own]["idm"], f"DELETE the run's {row}",
                                 "204 (or 404: never made)", detail, ok)

    def tokens(self, own: str, other: str, made: "Made") -> None:
        token = made.client_token()
        h = self.anon()
        calls = [("portal", "GET", "/api/v1/projects"), ("api", "GET", "/api/endpoints"),
                 ("mcp", "POST", "/api/mcp"), ("opendata", "GET", "/api/endpoints")]
        init = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "jc-smoke", "version": "1"}}}
        for role, method, path in calls:
            for org, refused in ((own, False), (other, True)):
                host = self.orgs[org].get(role)
                row = f"token.{own}.on.{org}.{role}"
                if not host:
                    self.r.skip(row, role, f"{method} {path}", "", f"jc.toml names no {role} host of {org}")
                    continue
                st, _, _ = h.call(method, f"{self.scheme}://{host}{path}", body=init if method == "POST" else None,
                                  headers={"Authorization": f"Bearer {token}", "Accept": "application/json, text/event-stream"})
                self.r.check(row, host, f"{method} {path} with a {own} realm token",
                             "401" if refused else "not 401 (the realm's own host)", str(st),
                             st == 401 if refused else st not in (0, 401))

    def codes(self, own: str, other: str, made: "Made") -> None:
        h = Http(self.guard)
        code, back = made.code(h)
        tok = self.url(other, "idm", f"/realms/{other}/protocol/openid-connect/token")
        st, _, raw = h.call("POST", tok, form={"grant_type": "authorization_code", "client_id": PORTAL_CLIENT,
                                               "code": code, "redirect_uri": back})
        err = (as_json(raw) or {}).get("error", "")
        self.r.check(f"code.{own}.at.{other}", self.orgs[other]["idm"], f"POST {urllib.parse.urlsplit(tok).path} "
                     f"with a {own} authorization code", "400 or 401, no token", f"{st} {err}".strip(), st in (400, 401))
        st, _, raw = h.call("POST", tok, form={"grant_type": "client_credentials", "client_id": made.client_id,
                                               "client_secret": made.client_secret})
        err = (as_json(raw) or {}).get("error", "")
        self.r.check(f"client.{own}.at.{other}", self.orgs[other]["idm"], f"POST {urllib.parse.urlsplit(tok).path} "
                     f"as {own}'s smoke client", "401 invalid_client", f"{st} {err}".strip(), st in (400, 401))

    def sessions(self, own: str, other: str, made: "Made") -> None:
        h = Http(self.guard, self.url(own, "portal", ""))
        ok, detail = R.sign_in(h, made.username, made.password)
        st, _, _ = h.call("GET", self.url(own, "portal", "/api/v1/projects"))
        self.r.check(f"session.{own}.on.{own}", self.orgs[own]["portal"], "GET /api/v1/projects with the session",
                     "200 (the control)", f"{st} ({detail})", ok and st == 200)
        st, _, _ = h.call("GET", self.url(other, "portal", "/api/v1/projects"))
        self.r.check(f"session.{own}.on.{other}", self.orgs[other]["portal"],
                     f"GET /api/v1/projects with {own}'s session cookie", "401", str(st), st == 401)
        url, st, page = R.follow(h, self.url(other, "portal", "/api/v1/auth/login"))
        at = urllib.parse.urlsplit(url)
        form = st == 200 and b"<form" in page and at.hostname == self.orgs[other]["idm"]
        self.r.check(f"session.{own}.signin.{other}", self.orgs[other]["portal"],
                     f"{other}'s sign-in with {own}'s cookies", f"{other}'s own form on {self.orgs[other]['idm']}",
                     f"{st} at {at.hostname}{at.path}", form and "code=" not in url)


class Made:
    """The smoke client and test person of one realm, made through its admin API and removed after."""

    def __init__(self, suite: Suite, realm: str):
        self.s, self.realm = suite, realm
        self.h = Http(suite.guard)
        self.base = suite.url(realm, "idm", f"/admin/realms/{realm}")
        tag = secrets.token_hex(4)
        self.client_id, self.username = f"jc-smoke-xr-{tag}", f"jc-smoke-xr-{tag}"
        self.client_secret = self.password = ""
        self.client_uuid = self.uid = None
        self.admin = ""

    def api(self, method, path, body=None):
        st, _, raw = self.h.call(method, self.base + path, body=body,
                                 headers={"Authorization": f"Bearer {self.admin}"})
        return st, as_json(raw)

    def make(self) -> None:
        secret = (self.s.admin_dir / self.realm).read_text().strip()
        tok = self.s.url(self.realm, "idm", f"/realms/{self.realm}/protocol/openid-connect/token")
        st, _, raw = self.h.call("POST", tok, form={"grant_type": "client_credentials",
                                                    "client_id": ADMIN_CLIENT, "client_secret": secret})
        self.admin = (as_json(raw) or {}).get("access_token") or ""
        if st != 200 or not self.admin:
            raise RuntimeError(f"{ADMIN_CLIENT} of {self.realm}: token {st}")
        self.client_secret = secrets.token_urlsafe(32)
        st, _ = self.api("POST", "/clients", {
            "clientId": self.client_id, "enabled": True, "publicClient": False, "secret": self.client_secret,
            "serviceAccountsEnabled": True, "standardFlowEnabled": False, "directAccessGrantsEnabled": False,
            "description": "jc-smoke crossrealm (T-7230): removed by the run"})
        if st not in (201, 204):
            raise RuntimeError(f"smoke client in {self.realm}: {st}")
        st, found = self.api("GET", f"/clients?clientId={self.client_id}")
        self.client_uuid = (found or [{}])[0].get("id") if isinstance(found, list) else None
        self.password = secrets.token_urlsafe(18) + "-7aZ"
        st, _ = self.api("POST", "/users", {"username": self.username, "enabled": True, "emailVerified": True,
                                            "email": f"{self.username}@smoke.invalid", "firstName": "Smoke",
                                            "lastName": "crossrealm"})
        if st not in (201, 204):
            raise RuntimeError(f"test person in {self.realm}: {st}")
        st, users = self.api("GET", f"/users?username={self.username}&exact=true")
        self.uid = (users or [{}])[0].get("id") if isinstance(users, list) else None
        st, _ = self.api("PUT", f"/users/{self.uid}/reset-password",
                         {"type": "password", "value": self.password, "temporary": False})
        if st not in (200, 204):
            raise RuntimeError(f"password of the test person in {self.realm}: {st}")
        SECOND_FACTOR.fresh(self.api, self.username, self.uid)

    def client_token(self) -> str:
        tok = self.s.url(self.realm, "idm", f"/realms/{self.realm}/protocol/openid-connect/token")
        st, _, raw = self.h.call("POST", tok, form={"grant_type": "client_credentials", "client_id": self.client_id,
                                                    "client_secret": self.client_secret})
        token = (as_json(raw) or {}).get("access_token")
        if st != 200 or not token:
            raise RuntimeError(f"smoke client token of {self.realm}: {st}")
        return token

    def code(self, h: Http) -> tuple[str, str]:
        """The test person's authorization code from the Portal's own sign-in on its host, taken
        before the Portal's callback redeems it: (code, redirect_uri)."""
        login_url, st, page = R.follow(h, self.s.url(self.realm, "portal", "/api/v1/auth/login"))
        if st != 200:
            raise RuntimeError(f"sign-in form of {self.realm}: {st}")
        text = page.decode(errors="replace")
        form = {"csrf_token": R._hidden(text, "csrf_token"), "session_code": R._hidden(text, "session_code"),
                "username": self.username, "password": self.password}
        st, hdrs, body = h.call("POST", login_url, form=form)
        st, hdrs, body = SECOND_FACTOR.answer(h.call, login_url, st, hdrs, body, self.username)
        loc = hdrs.get("location") or ""
        q = urllib.parse.parse_qs(urllib.parse.urlsplit(loc).query)
        if "code" not in q:
            raise RuntimeError(f"no code from {self.realm}'s sign-in: {st}")
        return q["code"][0], loc.split("?")[0]

    def remove(self) -> list[tuple[str, bool, str]]:
        out = []
        if not self.admin:
            return out
        for row, path in (("person", f"/users/{self.uid}" if self.uid else None),
                          ("client", f"/clients/{self.client_uuid}" if self.client_uuid else None)):
            if path is None:
                st = 404
            else:
                try:
                    st, _ = self.api("DELETE", path)
                except Refused as e:
                    out.append((row, False, str(e)))
                    continue
            out.append((row, st in (204, 404), str(st)))
        return out


def main(argv) -> int:
    p = argparse.ArgumentParser(prog="run crossrealm")
    p.add_argument("--out", required=True)
    p.add_argument("--base-url", help="ignored: the hosts come from --jc-config")
    p.add_argument("--jc-config", help="the deployment's jc.toml ([[organizations]] with hosts.idm)")
    p.add_argument("--realm-admin-dir", help="a file per Organization holding its realm's portal-reconciler secret")
    p.add_argument("--organizations", default="", help="comma list: the two Organizations the token rows use")
    a = p.parse_args(argv)
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    doc = tomllib.loads(Path(a.jc_config).read_text()) if a.jc_config else {}
    res = Results(SUITE, doc.get("base_host", a.base_url or ""))
    orgs = organizations(doc)
    for o, hs in list(orgs.items()):
        if missing := routed(hs):
            res.skip(f"routed.{o}", missing, "a DNS answer for every host of the Organization",
                     "its rows run", f"{missing} has no DNS answer: {o} is not routed yet")
            del orgs[o]
    if len(orgs) < 2:
        res.skip("one-realm", "jc", "a second Organization realm", "refused on the other's hosts", ONE_REALM)
        res.write(out, {"organizations": len(orgs)})
        return 0
    admin_dir = Path(a.realm_admin_dir) if a.realm_admin_dir else None
    pick = [o for o in a.organizations.split(",") if o]
    hosts = {h for hs in orgs.values() for h in hs.values()} | {urllib.parse.urlsplit(installation_login(doc)).hostname}
    admin = {o for o in orgs if admin_dir and (admin_dir / o).is_file()}
    log = out / "requests.log"
    log.write_text("")
    s = Suite(doc, res, Guard(hosts, admin, log), admin_dir, pick)
    s.orgs = orgs
    s.realms()
    s.one_realm()
    s.credentialed()
    res.write(out, {"organizations": ", ".join(orgs)})
    return 1 if res.totals()["fail"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
