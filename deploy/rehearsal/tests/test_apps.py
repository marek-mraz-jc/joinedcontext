"""Unit tests of deploy/rehearsal/apps.py (the rehearsal's Apps): pytest deploy/rehearsal/tests/test_apps.py"""

from __future__ import annotations

import base64
import hashlib
import http.server
import json
import sys
import threading
from pathlib import Path
from typing import ClassVar

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import apps

ROLES_APP = """apiVersion: joinedcontext.com/v1alpha1
kind: App
metadata: {name: alerts, namespace: helsinki}
spec:
  kind: ui
  visibility: roles
  lifecycle: published
  access:
    - role: viewer
      subjects: [{group: alerts-viewer}]
    - role: steward
      subjects: [{group: alerts-steward}, {user: someone}]
status:
  build: {digest: "sha256:%s", commit: c0ffee}
"""
DRAFT_APP = ROLES_APP.replace("name: alerts", "name: draft").replace("published", "draft") \
    .replace("alerts-viewer", "draft-viewer")


def export_with(tmp: Path, bundle: bytes | None, digest_of: bytes) -> Path:
    exp = tmp / "export"
    d = exp / "gitea" / "files" / "joinedcontext" / "helsinki" / "apps"
    (d / "alerts").mkdir(parents=True)
    (d / "alerts" / "app.yaml").write_text(ROLES_APP % hashlib.sha256(digest_of).hexdigest())
    (d / "draft").mkdir()
    (d / "draft" / "app.yaml").write_text(DRAFT_APP % ("0" * 64))
    if bundle is not None:
        (exp / "apps" / "alerts").mkdir(parents=True)
        (exp / "apps" / "alerts" / "bundle.tar.gz").write_bytes(bundle)
    return exp


def test_the_holder_gets_the_published_roles_apps_groups_and_no_credential_value(tmp_path):
    exp = export_with(tmp_path, None, b"x")
    realm = {"realm": "smartcity", "groups": [{"name": "alerts-viewer", "path": "/alerts-viewer"},
                                              {"name": "city", "path": "/city", "subGroups": [
                                                  {"name": "alerts-steward", "path": "/city/alerts-steward"}]}],
             "users": [{"username": "rehearsal-tester"}, {"username": apps.PERSON, "groups": ["/old"]}]}
    assert apps.role_groups(exp / "gitea" / "files") == ["alerts-steward", "alerts-viewer"]  # the draft's left out
    out, missing = apps.with_person(realm, ["alerts-steward", "alerts-viewer", "gone"], Path("/s/pw"))
    holders = [u for u in out["users"] if u["username"] == apps.PERSON]
    assert len(holders) == 1, "made again, never twice"
    assert holders[0]["groups"] == ["/alerts-viewer", "/city/alerts-steward"]
    assert holders[0]["credentials"][0]["value"] == "$(file:/s/pw)"
    assert missing == ["gone"]
    assert {u["username"] for u in out["users"]} == {"rehearsal-tester", apps.PERSON}


class Shako(http.server.BaseHTTPRequestHandler):
    seen: ClassVar[list] = []

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        Shako.seen.append((self.path, self.headers.get("Authorization"), body))
        held = [b for p, _, b in Shako.seen[:-1] if b["Version"] == body["Version"]]
        self.send_response(409 if held else 201)
        self.end_headers()
        self.wfile.write(b"{}")

    def do_GET(self):
        self.send_response(401)
        self.end_headers()

    def log_message(self, *a):
        pass


@pytest.fixture()
def shako():
    Shako.seen = []
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Shako)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://localhost:{srv.server_address[1]}"
    srv.shutdown()


def test_publish_sends_the_bytes_as_they_are_under_the_build_lanes_version(tmp_path, shako):
    """A tampered bundle is published unchanged: the Portal's fetch is what refuses it."""
    exp = export_with(tmp_path, b"tampered", digest_of=b"genuine")
    (tmp_path / "tok").write_text("shako-secret\n")
    report = tmp_path / "report.json"
    argv = ["publish", "--base", shako, "--export", str(exp), "--token-file", str(tmp_path / "tok"),
            "--report", str(report), "--wait", "5"]
    assert apps.main(argv) == 0
    path, auth, body = Shako.seen[0]
    assert path == "/git/odata/PublishPackage" and auth == "Bearer shako-secret"
    assert body["Owner"] == "jc-apps" and body["Package"] == "app-alerts" and body["File"] == "bundle.tar.gz"
    assert body["Version"] == "c0ffee-" + hashlib.sha256(b"genuine").hexdigest()[:12]
    assert base64.b64decode(body["ContentBase64"]) == b"tampered"
    row = json.loads(report.read_text())[0]
    assert row["result"] == "published" and row["matches"] is False
    assert "shako-secret" not in report.read_text()
    assert apps.main(argv) == 0  # again: 409, left alone
    assert json.loads(report.read_text())[0]["result"] == "unchanged"


def test_publish_writes_only_to_a_loopback_base(tmp_path):
    exp = export_with(tmp_path, b"b", b"b")
    (tmp_path / "tok").write_text("t")
    with pytest.raises(SystemExit):
        apps.main(["publish", "--base", "https://portal.smartcity.joinedcontext.com", "--export", str(exp),
                   "--token-file", str(tmp_path / "tok")])
