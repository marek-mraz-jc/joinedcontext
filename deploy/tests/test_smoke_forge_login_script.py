"""T-1422: scripts/smoke-forge-login.sh walks the forge's Keycloak login the way a browser
does. The suite runs the real script and the real curl against a local stub of the forge and
Keycloak, with a stub `kubectl` that hands out the password."""

import base64
import hashlib
import hmac
import struct
import time
import io
import os
import tarfile
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs

import pytest

SCRIPT = Path(__file__).resolve().parent.parent / "scripts" / "smoke-forge-login.sh"
PASSWORD = "s3cret-Pa55&word"
FOLDER = "/git/joinedcontext/configuration/src/branch/main/projects/helsinki"
# Layout 2 (PF-87, T-2647): the project is a repository of its own; the fake serves it as FOLDER.
PROJECT_REPO = "/git/joinedcontext/helsinki"
# What Gitea shows a member of `{slug}-writers` on a protected main: a form whose direct commit
# is disabled, so the only choice is a new branch.
PROTECTED_FORM = ('<div class="ui radio checkbox disabled"><input name="commit_choice" value="direct">'
                  "Not allowed to commit directly to branch because: User cannot push to branch</div>"
                  '<input name="commit_choice" value="commit-to-new-branch" checked>')
SEED = "journeySeed0123456789abcdefABCDEF"
OTP_FORM = ('<html><form id="kc-otp-login-form" action="{base}/kc/otp?session_code=c&amp;tab_id=d" '
            'method="post"></form>{error}</html>')
APP_NAME = "helsinki_map-alerts"
APPS_ORG = "joinedcontext-apps"
FORM = ('<html><form id="kc-form-login" onsubmit="return true;" '
        'action="{base}/kc/authenticate?session_code=a&amp;tab_id=b" method="post"></form>'
        '{error}</html>')


def totp(seed, at):
    """RFC 6238 HMAC-SHA256, 30 s, 6 digits over the seed's UTF-8 bytes; vector in the test below."""
    mac = hmac.new(seed.encode(), struct.pack(">Q", at // 30), hashlib.sha256).digest()
    offset = mac[-1] & 0x0F
    return str((struct.unpack(">I", mac[offset:offset + 4])[0] & 0x7FFFFFFF) % 10**6).zfill(6)


def archive():
    """What the forge's download link answers: the repository as a tar.gz with its files."""
    data = b"<html>map</html>"
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as tar:
        entry = tarfile.TarInfo("helsinki_map-alerts/index.html")
        entry.size = len(data)
        tar.addfile(entry, io.BytesIO(data))
    return buffer.getvalue()


def serve(mode):
    """mode: ok | pkce (the forge's button fails) | no-team (404 after login) | writable |
    forkable (the forge lets a reader fork) | app-hidden (the application repository is a 404
    to a reader) | app-public (anyone reads it) | app-empty (it holds no commit) | no-apps |
    apart (the Apps live in an organization of their own, T-2969) | apart-no-team (that
    organization has no readers team, T-3030) | apart-no-repos (it holds no repository while
    the project declares an App) | protected (layout 2: demo.steward gets the form of a protected
    main) | protected-all (so does demo.viewer) | otp (Keycloak asks demo.steward for a TOTP code,
    T-5120)."""
    forks = []
    org = APPS_ORG if mode.startswith("apart") else "joinedcontext"
    APP = f"/git/{org}/{APP_NAME}"
    reader = mode != "apart-no-team"

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def send(self, code, body="", headers=()):
            self.send_response(code)
            for k, v in headers:
                self.send_header(k, v)
            data = body if isinstance(body, bytes) else body.encode()
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def base(self):
            return f"http://{self.headers['Host']}"

        def do_GET(self):
            cookie = self.headers.get("Cookie") or ""
            signed_in = "forge=session" in cookie
            if self.path.startswith(PROJECT_REPO + "/"):
                self.path = self.path.replace(PROJECT_REPO + "/src/branch/main", FOLDER).replace(
                    PROJECT_REPO + "/_new/main", FOLDER.replace("/src/branch/", "/_new/")).replace(
                    PROJECT_REPO + "/fork", FOLDER.split("/src/branch/")[0] + "/fork")
            if self.path == "/git/user/oauth2/keycloak":
                if mode == "pkce":
                    return self.send(302, headers=[("Location", "/git/user/login")])
                return self.send(302, headers=[("Location", "/kc/auth")])
            if self.path == "/git/user/login":
                msg = "Missing parameter: code_challenge_method" if mode == "pkce" else ""
                return self.send(200, f'<div class="ui negative message flash-error"><p>{msg}</p></div>')
            if self.path == "/kc/auth":
                return self.send(200, FORM.format(base=self.base(), error=""))
            if self.path.startswith("/git/user/oauth2/keycloak/callback"):
                who = parse_qs(self.path.split("?", 1)[-1]).get("code", ["x"])[0]
                return self.send(302, headers=[("Location", "/git/"), ("Set-Cookie", f"forge=session-{who}; Path=/")])
            if self.path == "/git/":
                return self.send(200, "<html>dashboard</html>")
            if self.path == FOLDER:
                if not signed_in or mode == "no-team":
                    return self.send(404, "<html>Not found</html>")
                return self.send(200, '<a href="/git/joinedcontext/configuration/_new/main/projects/helsinki">Add File</a><a>project.yaml</a>')
            if self.path == FOLDER.replace("/src/branch/", "/_new/"):
                if mode == "protected-all" or (mode == "protected" and "steward" in cookie):
                    return self.send(200, PROTECTED_FORM)
                if mode == "writable":
                    return self.send(200, '<form><input name="commit_choice" value="direct"></form>')
                return self.send(200, "<p>You cannot edit this repository directly. Instead you can create a fork</p>")
            if self.path == FOLDER.split("/src/branch/")[0] + "/fork":
                return self.send(200, '<input type="hidden" name="_csrf" value="tok"><input name="uid" type="hidden" value="4">')
            if self.path == "/git/org/joinedcontext/teams/readers":
                return self.send(200 if signed_in else 404, "<html>readers</html>")
            if self.path == f"/git/org/{APPS_ORG}/teams/readers" and org == APPS_ORG:
                return self.send(200 if signed_in and reader else 404, "<html>readers</html>")
            if self.path == FOLDER + "/apps":
                if not signed_in:
                    return self.send(404)
                declared = "" if mode == "no-apps" else f'<a href="{FOLDER}/apps/map-alerts">map-alerts</a>'
                return self.send(200, f"<html>{declared}</html>")
            if self.path == "/git/joinedcontext":
                if not signed_in:
                    return self.send(404)
                listed = "" if mode == "no-apps" or org != "joinedcontext" else f'<a href="{APP}">{APP_NAME}</a>'
                return self.send(200, f'<a href="/git/joinedcontext/configuration">configuration</a>{listed}')
            if self.path == f"/git/{APPS_ORG}" and org == APPS_ORG:
                # A private organization is a 404 to anybody outside its teams.
                if not signed_in or not reader:
                    return self.send(404)
                listed = "" if mode == "apart-no-repos" else f'<a href="{APP}">{APP_NAME}</a>'
                return self.send(200, f"<html>{listed}</html>")
            if self.path == APP:
                if mode == "app-public" or (signed_in and reader and mode != "app-hidden"):
                    tree = "" if mode == "app-empty" else f'<a href="{APP}/src/branch/main/index.html">index.html</a>'
                    return self.send(200, f"<html>{tree}</html>")
                return self.send(404)
            if self.path == APP + "/archive/main.tar.gz" and signed_in and reader and mode not in ("app-hidden", "app-empty"):
                return self.send(200, archive())
            if self.path in ("/git/demo.steward/configuration", "/git/demo.viewer/configuration"):
                return self.send(200 if forks else 404, "<html>fork</html>")
            self.send(404)

        def do_POST(self):
            body = parse_qs(self.rfile.read(int(self.headers["Content-Length"])).decode())
            if self.path.endswith("/fork"):
                if mode == "forkable" and body.get("_csrf") == ["tok"] and body.get("uid") == ["4"]:
                    forks.append(self.path)
                    return self.send(303, headers=[("Location", "/git/")])
                return self.send(400, '{"errorMessage":"The owner has already reached the limit of 0 repositories."}')
            if self.path.startswith("/kc/otp"):
                # The code of this step or the one before, as Keycloak's look-ahead window of 1.
                now = int(time.time())
                if body.get("otp", [""])[0] in (totp(SEED, now), totp(SEED, now - 30)):
                    return self.send(302, headers=[("Location", "/git/user/oauth2/keycloak/callback?code=demo.steward")])
                err = '<span id="input-error-otp-code" class="kc-feedback">Invalid authenticator code.</span>'
                return self.send(200, OTP_FORM.format(base=self.base(), error=err))
            if body.get("password") == [PASSWORD] and body["username"][0].endswith("@hel.fi"):
                who = body["username"][0].split("@")[0]
                if mode == "otp" and who == "demo.steward":
                    return self.send(200, OTP_FORM.format(base=self.base(), error=""))
                return self.send(302, headers=[("Location", f"/git/user/oauth2/keycloak/callback?code={who}")])
            err = '<span id="input-error" class="kc-feedback">Invalid username or password.</span>'
            self.send(200, FORM.format(base=self.base(), error=err))

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def run(tmp_path, mode, password=PASSWORD, seed=SEED, **extra):
    server = serve(mode)
    kubectl = tmp_path / "kubectl"
    encoded = base64.b64encode(password.encode()).decode() if password else ""
    # The runner's Secret records the organization the App repositories live in (T-2969).
    owner = base64.b64encode((APPS_ORG if mode.startswith("apart") else "joinedcontext").encode()).decode()
    kubectl.write_text(
        '#!/bin/sh\ncase "$*" in\n'
        f'  *gitea-runner-registration*) printf %s "{owner}";;\n'
        f'  *data.totp*) printf %s "{base64.b64encode(seed.encode()).decode()}";;\n'
        f'  *secret*) printf %s "{encoded}";;\nesac\n'
    )
    kubectl.chmod(0o755)
    env = {**os.environ, "PATH": f"{tmp_path}:{os.environ['PATH']}", "JC_SMOKE_ORG": "hel.fi", **extra}
    try:
        return subprocess.run([str(SCRIPT), f"http://127.0.0.1:{server.server_port}"],
                              env=env, capture_output=True, text=True, timeout=60)
    finally:
        server.shutdown()


def test_both_demo_people_sign_in_and_read_without_write(tmp_path):
    r = run(tmp_path, "ok")
    assert r.returncode == 0, r.stdout + r.stderr
    assert "ok    an anonymous visitor does not read the configuration repository (404)" in r.stdout
    for user in ("demo.steward", "demo.viewer"):
        assert f"ok    {user} signs in to the forge through Keycloak" in r.stdout
        assert f"ok    {user} reads the configuration repository without write access" in r.stdout
    assert PASSWORD not in r.stdout + r.stderr


def test_a_wrong_password_fails_with_the_keycloak_message_never_the_password(tmp_path):
    r = run(tmp_path, "ok", password="wrong-pass")
    assert r.returncode == 1
    assert "Invalid username or password." in r.stdout
    assert "wrong-pass" not in r.stdout + r.stderr


def test_the_broken_button_fails_naming_the_forges_error(tmp_path):
    r = run(tmp_path, "pkce")
    assert r.returncode == 1
    assert "reaches no login form: Missing parameter: code_challenge_method" in r.stdout


def test_a_404_after_login_names_the_team_mapping(tmp_path):
    r = run(tmp_path, "no-team")
    assert r.returncode == 1
    assert "maps to no forge team" in r.stdout


def test_an_editor_in_the_forge_fails(tmp_path):
    r = run(tmp_path, "writable")
    assert r.returncode == 1
    assert "may commit to the configuration repository" in r.stdout


def test_a_project_writer_on_a_protected_main_passes_and_the_reader_still_has_no_form(tmp_path):
    r = run(tmp_path, "protected", JC_SMOKE_LAYOUT2="1")
    assert "demo.steward may propose a branch in the helsinki repository, main refuses their commit" in r.stdout
    assert "demo.viewer reads the helsinki repository without write access" in r.stdout


def test_a_project_writer_who_may_commit_to_main_fails(tmp_path):
    r = run(tmp_path, "writable", JC_SMOKE_LAYOUT2="1")
    assert r.returncode == 1
    assert "demo.steward may commit to the helsinki repository" in r.stdout


def test_a_reader_with_the_branch_form_fails_even_when_main_refuses(tmp_path):
    r = run(tmp_path, "protected-all", JC_SMOKE_LAYOUT2="1")
    assert r.returncode == 1
    assert "demo.viewer may commit to the helsinki repository" in r.stdout


def test_at_layout_1_the_protected_form_is_still_a_failure(tmp_path):
    r = run(tmp_path, "protected")
    assert r.returncode == 1
    assert "demo.steward may commit to the configuration repository" in r.stdout


def test_a_missing_secret_fails_instead_of_skipping(tmp_path):
    r = run(tmp_path, "ok", password="")
    assert r.returncode == 1
    assert "no password in Secret keycloak-user-demo-steward" in r.stdout


def test_an_anonymous_read_fails(tmp_path, monkeypatch):
    # The stub answers the folder to anyone holding the cookie; hand it to the anonymous call.
    monkeypatch.setenv("CURL_HOME", str(tmp_path))
    (tmp_path / ".curlrc").write_text('cookie = "forge=session"\n')
    r = run(tmp_path, "ok")
    assert "FAIL  an anonymous visitor reads the configuration repository" in r.stdout


def test_a_reader_who_can_fork_the_repository_fails(tmp_path):
    r = run(tmp_path, "forkable")
    assert r.returncode == 1
    assert "forked the configuration repository" in r.stdout


def test_a_refused_fork_passes(tmp_path):
    r = run(tmp_path, "ok")
    assert "ok    demo.viewer cannot fork the configuration repository (404)" in r.stdout


def test_every_application_repository_is_read_and_downloaded_by_a_reader_and_hidden_from_anyone(tmp_path):
    """T-2601, PF-79, AP-78: the owner met a 404 on an App's repository; the readers team holds
    every repository of the organization, so the source opens and downloads for both people."""
    r = run(tmp_path, "ok")
    assert r.returncode == 0, r.stdout + r.stderr
    assert "ok    an anonymous visitor does not read helsinki_map-alerts (404)" in r.stdout
    for user in ("demo.steward", "demo.viewer"):
        assert f"ok    {user} is in the forge team readers" in r.stdout
        assert f"ok    {user} reads and downloads application repository helsinki_map-alerts (1 files)" in r.stdout
    assert "not read configuration" not in r.stdout, "the configuration repository is no App"


def test_an_application_repository_a_reader_cannot_open_fails(tmp_path):
    r = run(tmp_path, "app-hidden")
    assert r.returncode == 1
    assert "FAIL  demo.viewer: application repository helsinki_map-alerts answers 404" in r.stdout


def test_an_application_repository_anyone_reads_fails(tmp_path):
    r = run(tmp_path, "app-public")
    assert r.returncode == 1
    assert "FAIL  an anonymous visitor reads helsinki_map-alerts (200)" in r.stdout


def test_an_application_repository_with_no_files_fails(tmp_path):
    r = run(tmp_path, "app-empty")
    assert r.returncode == 1
    assert "FAIL  demo.steward: application repository helsinki_map-alerts lists no files" in r.stdout


def test_a_forge_with_no_application_repository_says_so_and_passes(tmp_path):
    r = run(tmp_path, "no-apps")
    assert r.returncode == 0, r.stdout + r.stderr
    assert "ok    the forge holds no application repository yet" in r.stdout


def test_a_person_the_group_map_lands_nowhere_fails_on_the_team(tmp_path):
    r = run(tmp_path, "no-team")
    assert r.returncode == 1
    assert "the forge holds no application repository yet" not in r.stdout, (
        "with nobody signed in, no list of applications is claimed"
    )


def test_application_repositories_in_their_own_organization_are_read_by_a_reader(tmp_path):
    """T-2969, T-3030: the Apps moved to `joinedcontext-apps`; the smoke reads that organization,
    named by the runner Secret's `owner`, and a reader there reads and downloads every App."""
    r = run(tmp_path, "apart")
    assert r.returncode == 0, r.stdout + r.stderr
    for user in ("demo.steward", "demo.viewer"):
        assert f"ok    {user} is in the forge team readers of {APPS_ORG}" in r.stdout
        assert f"ok    {user} reads and downloads application repository {APP_NAME} (1 files)" in r.stdout
    assert f"ok    an anonymous visitor does not read {APP_NAME} (404)" in r.stdout


def test_an_applications_organization_without_readers_fails_and_claims_no_empty_forge(tmp_path):
    """T-3030: after the move no reader could open an App, and the smoke printed 'ok, the forge
    holds no application repository yet'."""
    r = run(tmp_path, "apart-no-team")
    assert r.returncode == 1, r.stdout + r.stderr
    assert f"FAIL  demo.viewer is not in the forge team readers of {APPS_ORG} (404)" in r.stdout
    assert "the forge holds no application repository yet" not in r.stdout
    assert f"FAIL  helsinki declares Apps (map-alerts) and no reader lists a repository of {APPS_ORG}" in r.stdout


def test_declared_apps_with_no_repository_fail(tmp_path):
    r = run(tmp_path, "apart-no-repos")
    assert r.returncode == 1, r.stdout + r.stderr
    assert f"FAIL  helsinki declares Apps (map-alerts) and no reader lists a repository of {APPS_ORG}" in r.stdout


def test_the_totp_helper_matches_the_rfc_6238_sha256_vector():
    # RFC 6238 appendix B, SHA-256 seed, T = 59 s: 46119246 (8 digits), 119246 in 6.
    assert totp("12345678901234567890123456789012", 59) == "119246"


def test_a_steward_asked_for_totp_answers_with_the_seed_and_never_prints_it(tmp_path):
    r = run(tmp_path, "otp")
    assert r.returncode == 0, r.stdout + r.stderr
    assert "ok    demo.steward signs in to the forge through Keycloak" in r.stdout
    assert SEED not in r.stdout + r.stderr


def test_a_wrong_seed_fails_with_the_keycloak_message(tmp_path):
    r = run(tmp_path, "otp", seed="another-seed-entirely")
    assert r.returncode == 1
    assert "Invalid authenticator code." in r.stdout


def test_a_missing_totp_seed_fails_naming_the_secret(tmp_path):
    r = run(tmp_path, "otp", seed="")
    assert r.returncode == 1
    assert "holds no totp seed" in r.stdout
