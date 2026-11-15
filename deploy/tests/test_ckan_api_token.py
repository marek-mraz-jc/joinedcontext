"""The catalogue API token Job (T-0493): its script against a fake CKAN and a fake Kubernetes API.

The fakes hold what the real ones decide: CKAN refuses an API call on a session without the CSRF
token and reads an unknown token as anonymous; the API server answers 404 for a Secret that is
not there. Nothing here needs a cluster.
"""

import base64
import json
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "charts/ckan/files/api-token.py"
PASSWORD = "admin-password-9f3"
# A token's shape without a token: built from parts, because a JWT-shaped literal is exactly what
# the history scan is there to catch.
MINTED = ".".join(["eyJhbGciOiJIUzI1NiJ9", "eyJqdGkiOiJuZXcifQ", "c2lnbmF0dXJl"])
HARVEST_MINTED = ".".join(["eyJhbGciOiJIUzI1NiJ9", "eyJqdGkiOiJoYXJ2ZXN0In0", "c2lnbmF0dXJl"])
ORGS = [{"name": "bbsk", "title": "BBSK", "description": "kraj"}, {"name": "banskabystrica", "title": "BB"}]


class FakeCkan:
    def __init__(self):
        self.tokens = {}  # token -> (id, name)
        self.calls = []
        self.users = {"ckan_admin": {"name": "ckan_admin", "sysadmin": True}}
        self.orgs = {"banskabystrica": {"name": "banskabystrica"}}
        self.members = set()  # (org, user, role)
        self.bearers = []  # the token each harvest-side call carried

    def handler(fake):
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def reply(self, status, body="", headers=()):
                self.send_response(status)
                for key, value in headers:
                    self.send_header(key, value)
                self.end_headers()
                self.wfile.write(body.encode())

            def session(self):
                return self.headers.get("Cookie", "")

            def do_GET(self):
                fake.calls.append(("GET", self.path))
                if self.path == "/api/3/action/status_show":
                    return self.reply(200, '{"success": true}')
                if self.path == "/user/login":
                    return self.reply(200, '<input type="hidden" name="_csrf_token" value="form-csrf"/>',
                                      [("Set-Cookie", "ckan=anon; Secure; HttpOnly")])
                if self.path == "/dataset/" and "ckan=signed-in" in self.session():
                    return self.reply(200, '<meta name="_csrf_token" content="page-csrf" />')
                return self.reply(404)

            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode()
                action = self.path.rsplit("/", 1)[-1]
                fake.calls.append(("POST", action))
                if self.path == "/user/login":
                    ok = ("ckan=anon" in self.session() and "_csrf_token=form-csrf" in body
                          and f"password={PASSWORD}" in body)
                    if not ok:
                        return self.reply(200, "Login failed")
                    return self.reply(302, headers=[("Location", "http://ckan/dashboard/datasets"),
                                                    ("Set-Cookie", "ckan=signed-in; Secure; HttpOnly")])
                payload = json.loads(body or "{}")
                bearer = self.headers.get("Authorization")
                on_session = "ckan=signed-in" in self.session() and self.headers.get("X-CSRFToken") == "page-csrf"
                if not (on_session or bearer in fake.tokens):
                    return self.reply(403, '{"success": false}')
                if action == "api_token_list":
                    result = [{"id": i, "name": n} for i, n in fake.tokens.values()]
                elif action == "api_token_revoke":
                    fake.tokens = {t: v for t, v in fake.tokens.items() if v[0] != payload["jti"]}
                    result = None
                elif action == "api_token_create":
                    minted = HARVEST_MINTED if payload["user"] == "harvest" else MINTED
                    fake.tokens[minted] = ("new-" + payload["user"], payload["name"])
                    result = {"token": minted}
                elif action in ("organization_show", "user_show"):
                    fake.bearers.append(bearer)
                    found = (fake.orgs if action == "organization_show" else fake.users).get(payload["id"])
                    if found is None:
                        return self.reply(404, '{"success": false}')
                    result = found
                elif action == "organization_create":
                    fake.orgs[payload["name"]] = payload
                    result = payload
                elif action == "user_create":
                    assert len(payload["password"]) >= 32
                    fake.users[payload["name"]] = result = {"name": payload["name"], "sysadmin": False}
                elif action == "organization_member_create":
                    fake.members.add((payload["id"], payload["username"], payload["role"]))
                    result = {}
                else:
                    return self.reply(404)
                return self.reply(200, json.dumps({"success": True, "result": result}))

        return Handler


class FakeKube:
    def __init__(self, token=None, **tokens):
        self.secrets = {name.replace("_", "-"): base64.b64encode(t.encode()).decode() for name, t in tokens.items()}
        if token is not None:
            self.secrets["ckan-api-token"] = base64.b64encode(token.encode()).decode()
        self.writes = []

    @property
    def secret(self):
        return self.secrets.get("ckan-api-token")

    def handler(fake):
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def reply(self, status, body=b""):
                self.send_response(status)
                self.end_headers()
                self.wfile.write(body)

            def name(self):
                return self.path.rsplit("/", 1)[-1]

            def do_GET(self):
                if self.name() not in fake.secrets:
                    return self.reply(404)
                return self.reply(200, json.dumps({"data": {"token": fake.secrets[self.name()]}}).encode())

            def do_PUT(self):
                return self.write(200 if self.name() in fake.secrets else 404)

            def do_POST(self):
                return self.write(201)

            def write(self, status):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                if status != 404:
                    fake.writes.append((self.command, self.path, body))
                    fake.secrets[body["metadata"]["name"]] = body["data"]["token"]
                return self.reply(status)

        return Handler


def serve(fake):
    server = ThreadingHTTPServer(("127.0.0.1", 0), fake.handler())
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def run(ckan, kube, tmp_path, password=PASSWORD, harvest=None):
    ckan_server, kube_server = serve(ckan), serve(kube)
    extra = {} if harvest is None else {"HARVEST_ORGS": json.dumps(harvest), "HARVEST_SECRET": "ckan-api-token-harvest",
                                        "HARVEST_USER": "harvest"}
    try:
        return subprocess.run(
            ["python3", "-c", SCRIPT.read_text()],
            env={"CATALOGUE": f"127.0.0.1:{ckan_server.server_port}", "ADMIN_USER": "ckan_admin",
                 "ADMIN_PASSWORD": password, "TOKEN_SECRET": "ckan-api-token", "TOKEN_NAME": "jcctl",
                 "NAMESPACE": "dev", "KUBE_API": f"http://127.0.0.1:{kube_server.server_port}",
                 "SA_DIR": str(tmp_path), **extra},
            capture_output=True, text=True, timeout=60,
        )
    finally:
        ckan_server.shutdown()
        kube_server.shutdown()


def test_a_first_install_mints_the_token_into_a_new_secret(tmp_path):
    ckan, kube = FakeCkan(), FakeKube()
    result = run(ckan, kube, tmp_path)
    assert result.returncode == 0, result.stderr
    assert [(method, path) for method, path, _ in kube.writes] == [("POST", "/api/v1/namespaces/dev/secrets")]
    written = kube.writes[0][2]
    assert written["metadata"] == {"name": "ckan-api-token", "namespace": "dev"}
    assert base64.b64decode(written["data"]["token"]).decode() == MINTED
    assert "minted jcctl into Secret ckan-api-token" in result.stdout
    for leak in (MINTED, PASSWORD, "page-csrf"):
        assert leak not in result.stdout + result.stderr


def test_a_token_that_still_authenticates_is_kept(tmp_path):
    ckan = FakeCkan()
    ckan.tokens[MINTED] = ("new", "jcctl")
    kube = FakeKube(token=MINTED)
    result = run(ckan, kube, tmp_path)
    assert result.returncode == 0, result.stderr
    assert "still authenticates" in result.stdout
    assert kube.writes == []
    assert ("POST", "api_token_create") not in ckan.calls


def test_a_stale_token_is_revoked_by_name_and_replaced(tmp_path):
    ckan = FakeCkan()
    ckan.tokens["eyJ.old.token"] = ("old", "jcctl")
    ckan.tokens["eyJ.other.token"] = ("other", "someone-else")
    kube = FakeKube(token="eyJ.lost.token")
    result = run(ckan, kube, tmp_path)
    assert result.returncode == 0, result.stderr
    assert "eyJ.old.token" not in ckan.tokens, "the previous jcctl token still works"
    assert "eyJ.other.token" in ckan.tokens, "a token of another name was revoked"
    assert [(method, path) for method, path, _ in kube.writes] == [("PUT", "/api/v1/namespaces/dev/secrets/ckan-api-token")]


def test_a_refused_sign_in_writes_nothing_and_fails_the_job(tmp_path):
    ckan, kube = FakeCkan(), FakeKube()
    result = run(ckan, kube, tmp_path, password="wrong")
    assert result.returncode != 0
    assert "could not sign in" in result.stderr
    assert kube.writes == [] and ckan.tokens == {}


def test_the_harvests_get_an_editor_token_and_never_the_administrators(tmp_path):
    """F-DEP1-002: the harvest user edits the harvested organizations, which the Job creates as
    the administrator; its own token lands in its own Secret."""
    ckan, kube = FakeCkan(), FakeKube()
    result = run(ckan, kube, tmp_path, harvest=ORGS)
    assert result.returncode == 0, result.stderr
    assert ckan.orgs["bbsk"]["title"] == "BBSK" and ckan.orgs["bbsk"]["description"] == "kraj"
    assert ckan.users["harvest"]["sysadmin"] is False
    assert ckan.members == {("bbsk", "harvest", "editor"), ("banskabystrica", "harvest", "editor")}
    written = {body["metadata"]["name"]: base64.b64decode(body["data"]["token"]).decode() for _, _, body in kube.writes}
    assert written == {"ckan-api-token": MINTED, "ckan-api-token-harvest": HARVEST_MINTED}
    assert set(ckan.bearers) == {MINTED}, "a harvest-side call ran on something but the administrator's token"
    for leak in (MINTED, HARVEST_MINTED, PASSWORD):
        assert leak not in result.stdout + result.stderr


def test_a_harvest_token_that_still_authenticates_is_kept(tmp_path):
    ckan = FakeCkan()
    ckan.tokens[MINTED] = ("new", "jcctl")
    ckan.tokens[HARVEST_MINTED] = ("h", "harvest")
    ckan.users["harvest"] = {"name": "harvest", "sysadmin": False}
    kube = FakeKube(token=MINTED, ckan_api_token_harvest=HARVEST_MINTED)
    result = run(ckan, kube, tmp_path, harvest=ORGS)
    assert result.returncode == 0, result.stderr
    assert "the token in ckan-api-token-harvest still authenticates" in result.stdout
    assert kube.writes == []
    assert ("POST", "api_token_create") not in ckan.calls


def test_a_harvest_user_that_became_an_administrator_gets_no_token(tmp_path):
    ckan = FakeCkan()
    ckan.users["harvest"] = {"name": "harvest", "sysadmin": True}
    kube = FakeKube()
    result = run(ckan, kube, tmp_path, harvest=ORGS)
    assert result.returncode != 0
    assert "is a site administrator" in result.stderr
    assert "ckan-api-token-harvest" not in kube.secrets
