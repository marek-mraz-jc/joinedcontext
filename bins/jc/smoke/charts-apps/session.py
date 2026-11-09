"""A signed-in Portal session for the charts-apps suite: a cookie jar, the test person (made with the
Portal's admin client, random password kept in memory only) and the browser-like sign-in."""

from __future__ import annotations

import html
import json
import re
import secrets
import urllib.error
import urllib.parse
import urllib.request

# The realm of the rehearsal (common.realm_of): callers pass `realm`, default jc's own.
REALM = "jc"
ADMIN_CLIENT = "portal-reconciler"
PERSON = "smoke-charts-apps"
# The imported realm's rehearsal persons (deploy/rehearsal: rehearsal.py add_tester, apps.py person).
TESTER = "rehearsal-tester"
HOLDER = "rehearsal-app-roles"


class Session:
    """Never follows redirects by itself; keeps cookies (Secure and __Host- ones too, as a browser
    does on http://localhost)."""

    def __init__(self, base: str, timeout: float = 60):
        self.base, self.timeout = base.rstrip("/"), timeout
        self.jar: dict[str, tuple[str, str]] = {}

        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *a, **k):
                return None

        self.opener = urllib.request.build_opener(NoRedirect)

    def url(self, path: str) -> str:
        return path if path.startswith("http") else self.base + path

    def _store(self, headers):
        for raw in headers.get_all("Set-Cookie") or []:
            parts = [p.strip() for p in raw.split(";")]
            if "=" not in parts[0]:
                continue
            name, value = parts[0].split("=", 1)
            path, expired = "/", False
            for attr in parts[1:]:
                low = attr.lower()
                if low.startswith("path="):
                    path = attr[5:]
                if low == "max-age=0":
                    expired = True
            if expired or not value:
                self.jar.pop(name, None)
            else:
                self.jar[name] = (value, path)

    def call(self, method, path, body=None, headers=None, form=None):
        headers = dict(headers or {})
        url = self.url(path)
        data = None
        if form is not None:
            data = urllib.parse.urlencode(form).encode()
            headers.setdefault("Content-Type", "application/x-www-form-urlencoded")
        elif body is not None:
            data = body if isinstance(body, bytes) else json.dumps(body).encode()
            headers.setdefault("Content-Type", "application/json")
        upath = urllib.parse.urlparse(url).path or "/"
        if cookies := "; ".join(f"{n}={v}" for n, (v, p) in self.jar.items() if upath.startswith(p)):
            headers.setdefault("Cookie", cookies)
        if method not in ("GET", "HEAD") and "__Host-jc_csrf" in self.jar:
            headers.setdefault("x-csrf-token", self.jar["__Host-jc_csrf"][0])
        req = urllib.request.Request(url, data=data, method=method, headers=headers)
        try:
            with self.opener.open(req, timeout=self.timeout) as r:
                self._store(r.headers)
                return r.status, {k.lower(): v for k, v in r.headers.items()}, r.read()
        except urllib.error.HTTPError as e:
            if e.headers:
                self._store(e.headers)
            return e.code, {k.lower(): v for k, v in (e.headers or {}).items()}, e.read() or b""
        except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
            return 0, {}, str(e).encode()

    def json(self, method, path, body=None, headers=None, form=None):
        st, _, raw = self.call(method, path, body, headers, form)
        try:
            return st, (json.loads(raw) if raw else None)
        except ValueError:
            return st, raw.decode(errors="replace")[:300]

    def follow(self, url, hops=12):
        for _ in range(hops):
            st, hdrs, body = self.call("GET", url)
            loc = hdrs.get("location")
            if st in (301, 302, 303, 307, 308) and loc:
                url = urllib.parse.urljoin(self.url(url), loc)
                continue
            return url, st, body
        raise RuntimeError(f"too many redirects from {url}")


def _hidden(page: str, name: str) -> str:
    m = re.search(rf'<input[^>]*name="{name}"[^>]*>', page)
    v = re.search(r'value="([^"]*)"', m.group(0)) if m else None
    if not v:
        raise RuntimeError(f"no {name} on the login page")
    return html.unescape(v.group(1))


def admin_token(s: Session, client_secret: str, realm: str = REALM) -> str:
    st, body = s.json("POST", f"/id/realms/{realm}/protocol/openid-connect/token", form={
        "grant_type": "client_credentials", "client_id": ADMIN_CLIENT, "client_secret": client_secret})
    if st != 200 or not isinstance(body, dict) or "access_token" not in body:
        raise RuntimeError(f"{ADMIN_CLIENT} token: {st}")
    return body["access_token"]


def make_person(s: Session, token: str, groups=("/platform-admins",), realm: str = REALM) -> str:
    """The suite's person in `realm`, made or reused; a fresh random password each run."""
    auth = {"Authorization": f"Bearer {token}"}
    password = secrets.token_urlsafe(18) + "-7aZ"
    st, body = s.json("POST", f"/id/admin/realms/{realm}/users", headers=auth, body={
        "username": PERSON, "enabled": True, "email": f"{PERSON}@smoke.invalid", "emailVerified": True,
        "firstName": "Smoke", "lastName": "Charts"})
    if st not in (201, 409):
        raise RuntimeError(f"create {PERSON}: {st} {str(body)[:200]}")
    st, users = s.json("GET", f"/id/admin/realms/{realm}/users?username={PERSON}&exact=true", headers=auth)
    uid = users[0]["id"]
    st, body = s.json("PUT", f"/id/admin/realms/{realm}/users/{uid}/reset-password", headers=auth,
                      body={"type": "password", "value": password, "temporary": False})
    if st not in (200, 204):
        raise RuntimeError(f"password of {PERSON}: {st}")
    st, tree = s.json("GET", f"/id/admin/realms/{realm}/groups?briefRepresentation=false", headers=auth)
    by_path, todo = {}, list(tree or [])
    while todo:
        g = todo.pop()
        by_path[g.get("path")] = g["id"]
        todo += g.get("subGroups") or []
    for path in groups:
        if path not in by_path:
            raise RuntimeError(f"group {path}: not in realm {realm}")
        st, _ = s.json("PUT", f"/id/admin/realms/{realm}/users/{uid}/groups/{by_path[path]}", headers=auth)
        if st not in (200, 204):
            raise RuntimeError(f"group {path} of {PERSON}: {st}")
    return password


def sign_in(s: Session, password: str) -> tuple[bool, str]:
    login_url, st, page = s.follow("/api/v1/auth/login")
    if st != 200:
        return False, f"GET /api/v1/auth/login -> {st}"
    text = page.decode(errors="replace")
    form = {"csrf_token": _hidden(text, "csrf_token"), "session_code": _hidden(text, "session_code"),
            "username": PERSON, "password": password}
    st, hdrs, _ = s.call("POST", login_url, form=form)
    loc = hdrs.get("location") or ""
    if st not in (302, 303) or "code=" not in loc:
        return False, f"POST login -> {st}"
    _, st, _ = s.follow(urllib.parse.urljoin(login_url, loc))
    return st < 400, f"callback -> {st}"


# ---- a person's token of one App's own client (what the edge holds for an App on prod) ----------

FILE_REF = re.compile(r"\$\(file:([^)]+)\)")


class Realm:
    """The realm file kaisatsu imported (<state>/realms/<realm>.json, 0600): an App client's secret
    and a rehearsal person's password, each read from the file its `$(file:...)` names."""

    def __init__(self, state):
        from pathlib import Path

        files = sorted((Path(state) / "realms").glob("*.json")) if state else []
        self.doc = json.loads(files[0].read_text()) if files else {}
        self.name = self.doc.get("realm", "")
        self.clients = {c.get("clientId"): c for c in self.doc.get("clients") or []}
        self.users = {u.get("username"): u for u in self.doc.get("users") or []}

    @staticmethod
    def _read(ref: str) -> str | None:
        from pathlib import Path

        m = FILE_REF.fullmatch((ref or "").strip())
        f = Path(m[1]) if m else None
        return f.read_text().strip() if f and f.is_file() else None

    def client(self, client_id: str) -> tuple[str | None, str | None]:
        """(secret, redirect URI) of a confidential client, or Nones."""
        c = self.clients.get(client_id) or {}
        uris = [u for u in c.get("redirectUris") or [] if u.startswith("https://")]
        redirect = uris[0].replace("/*", "/callback") if uris else None
        return self._read(c.get("secret", "")), redirect

    def password(self, username: str) -> str | None:
        creds = (self.users.get(username) or {}).get("credentials") or []
        return next((self._read(c.get("value", "")) for c in creds if c.get("type") == "password"), None)


def app_token(base: str, realm: Realm, client_id: str, username: str, timeout: float = 60) -> str:
    """An access token of `client_id` for `username`: the code flow with PKCE the edge runs for an
    App on prod, its redirect read from the Location header and never followed (it names the
    source environment's host). Raises RuntimeError naming the step that failed."""
    import base64
    import hashlib

    secret, redirect = realm.client(client_id)
    password = realm.password(username)
    if not (secret and redirect and password):
        raise RuntimeError(f"no {'secret' if not secret else 'redirect URI' if not redirect else 'password'} "
                           f"for {client_id if password else username} in the realm file")
    s = Session(base, timeout)
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    q = urllib.parse.urlencode({"client_id": client_id, "redirect_uri": redirect, "response_type": "code",
                                "scope": "openid", "state": secrets.token_hex(8), "code_challenge": challenge,
                                "code_challenge_method": "S256"})
    login_url, st, page = s.follow(f"/id/realms/{realm.name}/protocol/openid-connect/auth?{q}")
    if st != 200:
        raise RuntimeError(f"authorize {client_id}: {st}")
    text = page.decode(errors="replace")
    st, hdrs, _ = s.call("POST", login_url, form={"csrf_token": _hidden(text, "csrf_token"),
                                                  "session_code": _hidden(text, "session_code"),
                                                  "username": username, "password": password})
    loc = hdrs.get("location") or ""
    code = urllib.parse.parse_qs(urllib.parse.urlparse(loc).query).get("code", [None])[0]
    if st not in (302, 303) or not code or not loc.startswith(redirect):
        raise RuntimeError(f"login of {username} at {client_id}: {st}")
    st, body = s.json("POST", f"/id/realms/{realm.name}/protocol/openid-connect/token", form={
        "grant_type": "authorization_code", "code": code, "redirect_uri": redirect, "client_id": client_id,
        "client_secret": secret, "code_verifier": verifier})
    if st != 200 or not isinstance(body, dict) or "access_token" not in body:
        raise RuntimeError(f"token of {username} at {client_id}: {st}")
    return body["access_token"]
