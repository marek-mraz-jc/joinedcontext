"""The suite's one person: a test administrator in the Portal realm `jc` (group /platform-admins),
signed in to the Portal the way the browser does it, so every Portal call carries its session
cookie and the CSRF echo. Made at the start with a random password kept in memory, deleted at the
end. Named smoke-pipelines-*; nothing secret reaches the results."""

from __future__ import annotations

import html
import json
import re
import secrets
import urllib.error
import urllib.parse
import urllib.request
from http.cookies import SimpleCookie
from pathlib import Path

from common import Http, as_json

REALM = "jc"
ADMIN_CLIENT = "portal-reconciler"
GROUP = "/platform-admins"
CSRF_COOKIE = "__Host-jc_csrf"


class Browser(Http):
    """Http with a cookie jar (every cookie kept whatever its Secure flag: the rehearsal is http)
    and the Portal's double-submit CSRF header on every unsafe call."""

    def __init__(self, base: str, timeout: float = 60):
        super().__init__(base, timeout)
        self.cookies: dict[str, str] = {}

    def call(self, method, path, token=None, headers=None, body=None, form=None):
        headers = dict(headers or {})
        if self.cookies:
            headers["Cookie"] = "; ".join(f"{k}={v}" for k, v in self.cookies.items())
            if method not in ("GET", "HEAD") and CSRF_COOKIE in self.cookies:
                headers["X-CSRF-Token"] = self.cookies[CSRF_COOKIE]
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
                c = SimpleCookie()
                try:
                    c.load(line)
                except Exception:  # noqa: BLE001 - a cookie the stdlib cannot read is not ours
                    continue
                for k, m in c.items():
                    if m["max-age"] == "0" or not m.value:
                        self.cookies.pop(k, None)
                    else:
                        self.cookies[k] = m.value
            return getattr(r, "status", None) or r.code, {k.lower(): v for k, v in r.headers.items()}, raw


def _hidden(page: str, name: str) -> str:
    m = re.search(rf'<input[^>]*name="{name}"[^>]*>', page)
    v = re.search(r'value="([^"]*)"', m.group(0)) if m else None
    if not v:
        raise RuntimeError(f"no {name} on the login page")
    return html.unescape(v.group(1))


def _follow(b: Browser, url: str, hops: int = 12):
    for _ in range(hops):
        st, h, body = b.call("GET", url)
        loc = h.get("location")
        if st in (301, 302, 303, 307, 308) and loc:
            url = urllib.parse.urljoin(url if url.startswith("http") else b.base + url, loc)
            continue
        return url, st, body
    raise RuntimeError(f"too many redirects from {url}")


class Admin:
    """The signed-in test administrator; `close()` deletes the person again."""

    def __init__(self, base: str, state: Path):
        self.http = Http(base)
        self.browser = Browser(base)
        self.username = "smoke-pipelines-" + secrets.token_hex(3)
        self.uid = None
        secret = (state / "portal-admin-client-secret").read_text().strip()
        form = urllib.parse.urlencode({"grant_type": "client_credentials", "client_id": ADMIN_CLIENT,
                                       "client_secret": secret}).encode()
        st, _, raw = self.http.call("POST", f"/id/realms/{REALM}/protocol/openid-connect/token",
                                    headers={"Content-Type": "application/x-www-form-urlencoded"}, body=form)
        doc = as_json(raw) or {}
        if st != 200 or "access_token" not in doc:
            raise RuntimeError(f"{ADMIN_CLIENT} token: {st}")
        self.admin = doc["access_token"]

    def _api(self, method, path, body=None):
        st, _, raw = self.http.call(method, f"/id/admin/realms/{REALM}{path}", self.admin, body=body)
        return st, as_json(raw)

    def sign_in(self) -> None:
        password = secrets.token_urlsafe(18) + "-7aZ"
        st, body = self._api("POST", "/users", {
            "username": self.username, "enabled": True, "emailVerified": True,
            "email": f"{self.username}@rehearsal.invalid", "firstName": "Smoke", "lastName": "Pipelines"})
        if st not in (201, 409):
            raise RuntimeError(f"create {self.username}: {st} {body}")
        st, users = self._api("GET", f"/users?username={self.username}&exact=true")
        self.uid = users[0]["id"]
        st, body = self._api("PUT", f"/users/{self.uid}/reset-password",
                             {"type": "password", "value": password, "temporary": False})
        if st not in (200, 204):
            raise RuntimeError(f"password of {self.username}: {st}")
        st, tree = self._api("GET", "/groups?briefRepresentation=false")
        todo, gid = list(tree or []), None
        while todo:
            g = todo.pop()
            if g.get("path") == GROUP:
                gid = g["id"]
            todo += g.get("subGroups") or []
        if not gid:
            raise RuntimeError(f"no group {GROUP} in realm {REALM}")
        st, _ = self._api("PUT", f"/users/{self.uid}/groups/{gid}")
        if st not in (200, 204):
            raise RuntimeError(f"group {GROUP}: {st}")
        login_url, st, page = _follow(self.browser, "/api/v1/auth/login")
        if st != 200:
            raise RuntimeError(f"GET /api/v1/auth/login -> {st}")
        text = page.decode(errors="replace")
        form = {"csrf_token": _hidden(text, "csrf_token"), "session_code": _hidden(text, "session_code"),
                "username": self.username, "password": password}
        st, h, _ = self.browser.call("POST", login_url, form=form)
        loc = h.get("location") or ""
        if st not in (302, 303) or "code=" not in loc:
            raise RuntimeError(f"login: {st}")
        _, st, _ = _follow(self.browser, urllib.parse.urljoin(login_url, loc))
        if st >= 400:
            raise RuntimeError(f"Portal callback: {st}")

    def close(self) -> None:
        if self.uid:
            self._api("DELETE", f"/users/{self.uid}")
