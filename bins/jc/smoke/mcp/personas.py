"""The three administrators of MCP-ADMIN.md as real people of the rehearsal: a test person each in
kaisatsu's Portal realm, a RoleBinding each in the configuration repository (through shako, the
way a merged Change lands), and each person's own token from the MCP client `mcp-local`
(authorization code + PKCE, scope `mcp:portal`), as Claude Code would hold it.

Everything this writes is named `smoke-mcp-*` and is removed again by `cleanup()`. Passwords and
tokens stay in memory; nothing secret is written to the results."""

from __future__ import annotations

import base64
import hashlib
import html
import json
import re
import secrets
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from http.cookies import SimpleCookie
from pathlib import Path

import yaml

from common import Http

REALM = "jc"
ADMIN_CLIENT = "portal-reconciler"
MCP_CLIENT = "mcp-local"
REDIRECT = "http://localhost/callback"
BINDINGS_DIR = "users/assignments"


class Browser(Http):
    """Http with a cookie jar (every cookie kept whatever its Secure flag: the rehearsal is http)."""

    def __init__(self, base: str, timeout: float = 60):
        super().__init__(base, timeout)
        self.cookies: dict[str, str] = {}

    def call(self, method, path, token=None, headers=None, body=None, form=None):
        headers = dict(headers or {})
        if self.cookies:
            headers["Cookie"] = "; ".join(f"{k}={v}" for k, v in self.cookies.items())
        if token:
            headers["Authorization"] = f"Bearer {token}"
        data = None
        if form is not None:
            data = urllib.parse.urlencode(form).encode()
            headers["Content-Type"] = "application/x-www-form-urlencoded"
        elif body is not None:
            data = body if isinstance(body, bytes) else json.dumps(body).encode()
            headers.setdefault("Content-Type", "application/json")
        url = path if path.startswith("http") else self.base + path
        req = urllib.request.Request(url, data=data, method=method, headers=headers)
        try:
            r = self.opener.open(req, timeout=self.timeout)
        except urllib.error.HTTPError as e:
            r = e
        except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
            return 0, {}, str(e).encode()
        with r:
            raw = r.read() or b""
            for line in r.headers.get_all("Set-Cookie") or []:
                self._keep(line)
            return r.status if hasattr(r, "status") else r.code, \
                {k.lower(): v for k, v in r.headers.items()}, raw

    def _keep(self, line: str) -> None:
        c = SimpleCookie()
        try:
            c.load(line)
        except Exception:  # noqa: BLE001 - a cookie the stdlib cannot read is not ours
            return
        for k, m in c.items():
            if m["max-age"] == "0" or not m.value:
                self.cookies.pop(k, None)
            else:
                self.cookies[k] = m.value


def _hidden(page: str, name: str) -> str:
    m = re.search(rf'<input[^>]*name="{name}"[^>]*>', page)
    v = re.search(r'value="([^"]*)"', m.group(0)) if m else None
    if not v:
        raise RuntimeError(f"no {name} on the login page")
    return html.unescape(v.group(1))


def _follow(b: Browser, url: str, stop: str | None = None, hops: int = 12):
    """GETs and follows redirects on this base; a Location starting with `stop` is returned unread."""
    if not url.startswith("http"):
        url = b.base + url
    for _ in range(hops):
        st, h, body = b.call("GET", url)
        loc = h.get("location")
        if st in (301, 302, 303, 307, 308) and loc:
            nxt = urllib.parse.urljoin(url, loc)
            if stop and nxt.startswith(stop):
                return nxt, st, body
            url = nxt
            continue
        return url, st, body
    raise RuntimeError(f"too many redirects from {url}")


def _login(b: Browser, page_url: str, page: bytes, username: str, password: str) -> str:
    """Posts kaisatsu's login form; answers the Location it redirects to."""
    text = page.decode(errors="replace")
    form = {"csrf_token": _hidden(text, "csrf_token"), "session_code": _hidden(text, "session_code"),
            "username": username, "password": password}
    st, h, body = b.call("POST", page_url, form=form)
    loc = h.get("location") or ""
    if st not in (302, 303) or "code=" not in loc:
        raise RuntimeError(f"login of {username}: {st} {body[:160]!r}")
    return loc


@dataclass
class Person:
    key: str              # platform | organization | project
    username: str
    role: str
    scope: dict
    uid: str = ""
    password: str = field(default="", repr=False)
    token: str | None = field(default=None, repr=False)


class Fixture:
    def __init__(self, base: str, state: Path, config_repo: str, log=print):
        self.base, self.state, self.log = base, state, log
        self.owner, self.repo = config_repo.split("/", 1)
        self.http = Http(base)
        self.people: dict[str, Person] = {}
        self.files: list[str] = []
        self.connectors: list[tuple[Browser, str]] = []

    # -------------------------------------------------------------- secrets of the rehearsal

    def _secret(self, name: str) -> str:
        return (self.state / name).read_text().strip()

    def admin_token(self) -> str:
        form = urllib.parse.urlencode({"grant_type": "client_credentials", "client_id": ADMIN_CLIENT,
                                       "client_secret": self._secret("portal-admin-client-secret")}).encode()
        st, _, raw = self.http.call("POST", f"/id/realms/{REALM}/protocol/openid-connect/token",
                                    headers={"Content-Type": "application/x-www-form-urlencoded"}, body=form)
        if st != 200:
            raise RuntimeError(f"{ADMIN_CLIENT} token: {st}")
        return json.loads(raw)["access_token"]

    # -------------------------------------------------------------- people

    def make_person(self, p: Person, admin: str) -> None:
        p.password = secrets.token_urlsafe(18) + "-7aZ"
        st, _, raw = self.http.call("POST", f"/id/admin/realms/{REALM}/users", token=admin, body={
            "username": p.username, "enabled": True, "email": f"{p.username}@rehearsal.invalid",
            "emailVerified": True, "firstName": "Smoke", "lastName": p.key})
        if st not in (201, 409):
            raise RuntimeError(f"create {p.username}: {st} {raw[:160]!r}")
        st, _, raw = self.http.call("GET", f"/id/admin/realms/{REALM}/users?username={p.username}&exact=true",
                                    token=admin)
        p.uid = json.loads(raw)[0]["id"]
        st, _, raw = self.http.call("PUT", f"/id/admin/realms/{REALM}/users/{p.uid}/reset-password", token=admin,
                                    body={"type": "password", "value": p.password, "temporary": False})
        if st not in (200, 204):
            raise RuntimeError(f"password of {p.username}: {st}")

    def mcp_token(self, p: Person, scope: str = "mcp:portal") -> str:
        """The person's own token from `mcp-local` by code + PKCE: the code is read from the
        redirect to the loopback callback, never sent anywhere."""
        b = Browser(self.base)
        verifier = secrets.token_urlsafe(48)
        challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
        q = urllib.parse.urlencode({"client_id": MCP_CLIENT, "redirect_uri": REDIRECT, "response_type": "code",
                                    "scope": f"openid {scope}", "state": "smoke", "code_challenge": challenge,
                                    "code_challenge_method": "S256"})
        url, st, page = _follow(b, f"/id/realms/{REALM}/protocol/openid-connect/auth?{q}", stop=REDIRECT)
        if st != 200:
            raise RuntimeError(f"authorize for {p.username}: {st} {page[:160]!r}")
        loc = _login(b, url, page, p.username, p.password)
        if not loc.startswith(REDIRECT):
            # A consent page or another hop inside the realm first.
            loc, _, _ = _follow(b, urllib.parse.urljoin(url, loc), stop=REDIRECT)
        code = urllib.parse.parse_qs(urllib.parse.urlparse(loc).query).get("code", [""])[0]
        form = urllib.parse.urlencode({"grant_type": "authorization_code", "client_id": MCP_CLIENT, "code": code,
                                       "redirect_uri": REDIRECT, "code_verifier": verifier}).encode()
        st, _, raw = self.http.call("POST", f"/id/realms/{REALM}/protocol/openid-connect/token",
                                    headers={"Content-Type": "application/x-www-form-urlencoded"}, body=form)
        if st != 200:
            raise RuntimeError(f"token for {p.username} ({scope}): {st} {raw[:200]!r}")
        return json.loads(raw)["access_token"]

    def portal_session(self, p: Person) -> Browser:
        """The person signed in to the Portal in a browser of its own (session cookie + CSRF)."""
        b = Browser(self.base)
        url, st, page = _follow(b, "/api/v1/auth/login")
        if st != 200:
            raise RuntimeError(f"Portal login page: {st} at {url}")
        loc = _login(b, url, page, p.username, p.password)
        _, st, _ = _follow(b, urllib.parse.urljoin(url, loc))
        if st >= 400:
            raise RuntimeError(f"Portal callback for {p.username}: {st}")
        return b

    # -------------------------------------------------------------- bindings in the forge

    def _shako(self, method, path, body=None):
        return self.http.call(method, f"/git/api/v1{path}", headers={
            "Authorization": f"token {self._secret('shako-token')}"}, body=body)

    def _held(self, path: str) -> str | None:
        st, _, raw = self._shako("GET", f"/repos/{self.owner}/{self.repo}/git/trees/main?recursive=true&per_page=1000")
        if st != 200:
            raise RuntimeError(f"shako tree of {self.owner}/{self.repo}: {st}")
        for e in json.loads(raw).get("tree", []):
            if e.get("path") == path:
                return e.get("sha")
        return None

    def _commit(self, ops: list, message: str) -> None:
        st, _, raw = self._shako("POST", f"/projects/{self.owner}/repositories/{self.repo}/contents",
                                 body={"branch": "main", "files": ops, "message": message})
        if st not in (200, 201):
            raise RuntimeError(f"shako commit to {self.owner}/{self.repo}: {st} {raw[:200]!r}")

    def bind(self) -> None:
        ops = []
        for p in self.people.values():
            path = f"{BINDINGS_DIR}/{p.username}.yaml"
            doc = {"apiVersion": "joinedcontext.com/v1alpha1", "kind": "RoleBinding",
                   "metadata": {"name": p.username, "namespace": "org"},
                   "spec": {"subjects": [{"user": p.uid}], "role": p.role, "scope": p.scope}}
            content = base64.b64encode(("# jc smoke suite mcp, removed after the run\n" +
                                        yaml.safe_dump(doc, sort_keys=False)).encode()).decode()
            sha = self._held(path)
            op = {"operation": "update" if sha else "create", "path": path, "content": content}
            if sha:
                op["sha"] = sha
            ops.append(op)
            self.files.append(path)
        self._commit(ops, "smoke mcp: bind the three administrator personas")

    def setup(self, plan: dict) -> None:
        admin = self.admin_token()
        stamp = secrets.token_hex(3)
        for key, (role, scope) in plan.items():
            p = Person(key, f"smoke-mcp-{key}-{stamp}", role, scope)
            self.make_person(p, admin)
            self.people[key] = p
        self.bind()
        for p in self.people.values():
            p.token = self.mcp_token(p)

    # -------------------------------------------------------------- connectors

    def create_connector(self, owner: Person, body: dict) -> tuple[int, dict | str]:
        b = self.portal_session(owner)
        # The CSRF cookie is minted by any read of the session.
        b.call("GET", "/api/v1/auth/me")
        csrf = b.cookies.get("__Host-jc_csrf", "")
        st, _, raw = b.call("POST", "/api/v1/connectors", headers={"X-CSRF-Token": csrf}, body=body)
        try:
            out = json.loads(raw)
        except ValueError:
            out = raw[:200].decode(errors="replace")
        if st == 201 and isinstance(out, dict) and out.get("id"):
            self.connectors.append((b, out["id"]))
        return st, out

    def revoke_connector(self, cid: str) -> int:
        for b, c in self.connectors:
            if c == cid:
                csrf = b.cookies.get("__Host-jc_csrf", "")
                st, _, _ = b.call("DELETE", f"/api/v1/connectors/{cid}", headers={"X-CSRF-Token": csrf})
                return st
        return 0

    # -------------------------------------------------------------- cleanup

    def cleanup(self) -> list[str]:
        problems = []
        for b, cid in self.connectors:
            try:
                self.revoke_connector(cid)
            except Exception as e:  # noqa: BLE001
                problems.append(f"connector {cid}: {e}")
        try:
            ops = [{"operation": "delete", "path": f, "sha": sha} for f in self.files if (sha := self._held(f))]
            if ops:
                self._commit(ops, "smoke mcp: remove the persona bindings")
        except Exception as e:  # noqa: BLE001
            problems.append(f"bindings: {e}")
        try:
            admin = self.admin_token()
            for p in self.people.values():
                if p.uid:
                    self.http.call("DELETE", f"/id/admin/realms/{REALM}/users/{p.uid}", token=admin)
        except Exception as e:  # noqa: BLE001
            problems.append(f"people: {e}")
        return problems


def wait_until(pred, seconds: float, every: float = 3.0):
    end = time.time() + seconds
    while True:
        v = pred()
        if v or time.time() > end:
            return v
        time.sleep(every)
