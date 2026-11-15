"""scripts/alert-notes.py: Alertmanager alerts become one note line when they start and one when
they resolve, carrying only allow-listed labels (T-5104)."""

import importlib.util
import json
import socket
import subprocess
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/alert-notes.py"
spec = importlib.util.spec_from_file_location("alert_notes", SCRIPT)
alert_notes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(alert_notes)


def alert(fingerprint, name, state="active", **labels):
    return {"fingerprint": fingerprint, "status": {"state": state},
            "labels": {"alertname": name, "severity": "critical", **labels},
            "annotations": {"summary": f"{name} summary"}}


def test_an_alert_is_noted_once_when_it_fires_and_once_when_it_resolves():
    down = alert("a1", "ProbeSurfaceDown", target="dev", surface="odata", endpoint="parking")
    lines, state = alert_notes.notes([down], {})
    assert lines == ["FIRING ProbeSurfaceDown [critical] endpoint=parking surface=odata target=dev: ProbeSurfaceDown summary"]
    assert alert_notes.notes([down], state)[0] == []
    lines, state = alert_notes.notes([], state)
    assert lines == ["RESOLVED ProbeSurfaceDown [critical] endpoint=parking surface=odata target=dev: ProbeSurfaceDown summary"]
    assert state == {}


def test_silenced_and_inhibited_alerts_are_not_noted():
    assert alert_notes.notes([alert("a1", "NodeMemoryHigh", state="suppressed")], {})[0] == []


def test_only_allow_listed_labels_reach_a_note():
    leaky = alert("a2", "X", target="dev", user="jane@example.org", client_ip="198.51.100.7", authorization="Bearer abc")
    (text,), _ = alert_notes.notes([leaky], {})
    assert "jane" not in text and "198.51" not in text and "Bearer" not in text
    assert "target=dev" in text


def test_the_script_keeps_its_state_between_runs(tmp_path):
    state = tmp_path / "state.json"
    alerts = json.dumps([alert("a3", "TLSCertificateExpiresSoon", host="api.dev.example.org")])

    def run(stdin):
        return subprocess.run([sys.executable, str(SCRIPT), str(state)], input=stdin, capture_output=True, text=True, check=True).stdout

    assert run(alerts).startswith("FIRING TLSCertificateExpiresSoon [critical] host=api.dev.example.org")
    assert run(alerts).strip() == ""
    assert run("[]").startswith("RESOLVED TLSCertificateExpiresSoon")


WATCH = SCRIPT.parent / "alert-watch.sh"

# kubectl's port-forward, played by a server on the forwarded port that answers like Alertmanager.
KUBECTL = """#!{python}
import http.server, os, sys
stub = os.environ["STUB_DIR"]
open(os.path.join(stub, "kubectl.args"), "w").write(" ".join(sys.argv[1:]))
if os.environ.get("STUB_FAIL"):
    sys.exit(1)
body = open(os.path.join(stub, "alerts.json"), "rb").read()
class Alertmanager(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        found = self.path == "/api/v2/alerts"
        self.send_response(200 if found else 404)
        self.end_headers()
        self.wfile.write(body if found else b"")
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[-1].split(":")[0])), Alertmanager).serve_forever()
"""


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def watch(tmp_path, alerts, fetch_fails=False):
    """Runs scripts/alert-watch.sh with a stub kubectl and a stub tasks/note; returns the notes."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    (tmp_path / "alerts.json").write_text(alerts)
    kubectl = bin_dir / "kubectl"
    kubectl.write_text(KUBECTL.format(python=sys.executable))
    note = bin_dir / "note"
    note.write_text("#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$STUB_DIR/notes\"\n")
    kubectl.chmod(0o755)
    note.chmod(0o755)
    env = {"PATH": f"{bin_dir}:/usr/bin:/bin", "STUB_DIR": str(tmp_path), "TASKS_NOTE": str(note),
           "ALERT_WATCH_PORT": str(free_port()), **({"STUB_FAIL": "1"} if fetch_fails else {})}
    result = subprocess.run(["bash", str(WATCH), "dev", str(tmp_path / "state.json")], env=env,
                            capture_output=True, text=True, timeout=60)
    notes = tmp_path / "notes"
    return result.returncode, notes.read_text().splitlines() if notes.exists() else []


def test_the_watch_posts_each_change_once_to_the_deployer(tmp_path):
    firing = json.dumps([alert("a4", "NodeMemoryHigh", instance="node-1")])
    code, notes = watch(tmp_path, firing)
    assert code == 0
    assert notes == ["@deployer FIRING NodeMemoryHigh [critical] instance=node-1: NodeMemoryHigh summary"]
    args = (tmp_path / "kubectl.args").read_text().split()
    assert args[:5] == ["-n", "dev", "port-forward", "--address", "127.0.0.1"]
    assert args[5] == "svc/alertmanager-operated" and args[6].endswith(":9093")
    assert watch(tmp_path, firing)[1] == notes, "an unchanged alert is posted again"
    assert watch(tmp_path, "[]")[1][-1].startswith("@deployer RESOLVED NodeMemoryHigh")


def test_a_failed_fetch_posts_nothing_and_keeps_the_state(tmp_path):
    watch(tmp_path, json.dumps([alert("a5", "VolumeAlmostFull", persistentvolumeclaim="data")]))
    state = (tmp_path / "state.json").read_text()
    code, notes = watch(tmp_path, "[]", fetch_fails=True)
    assert code != 0
    assert len(notes) == 1, "a failed fetch posted a note"
    assert (tmp_path / "state.json").read_text() == state
