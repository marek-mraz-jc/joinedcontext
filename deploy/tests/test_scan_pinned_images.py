"""T-3289: every image the deployment pins is scanned, and a fixable CRITICAL (or a fixable HIGH
CISA lists as exploited) fails the job. The fixture is trivy's own report of the Gitea pin before
T-3286 (1.27.0-rootless), which carries CVE-2026-60004 fixed in 1.27.1."""

import datetime
import importlib.util
import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "tests/fixtures/pinned-images/gitea-1.27.0-trivy.json"
_spec = importlib.util.spec_from_file_location("scan_pinned_images", ROOT / "scripts/scan-pinned-images.py")
scan = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(scan)

TODAY = datetime.date(2026, 10, 7)


def test_every_pin_of_every_component_is_listed_by_digest():
    images = scan.pinned()
    assert len(images) >= 20
    assert all("@sha256:" in image for image in images), images
    assert "docker.gitea.com/gitea@sha256:1c17ecaead42eb3b5391553d8708103a4beb0e86edf5b9ebc1eb269c318845f2" in images, "the chart's registry joins its repository"


def test_the_known_vulnerable_gitea_pin_fails_on_its_fixable_critical(tmp_path):
    found = scan.judge([FIXTURE], None, tmp_path / "none.yaml", TODAY)
    assert any("CVE-2026-60004 CRITICAL" in line and "fixed in 1.27.1" in line for line in found), found


def test_a_fixable_high_blocks_only_when_it_is_known_exploited(tmp_path):
    report = {"ArtifactName": "example.org/x@sha256:1", "Results": [{"Vulnerabilities": [
        {"VulnerabilityID": "CVE-2026-1", "Severity": "HIGH", "FixedVersion": "2", "PkgName": "p", "InstalledVersion": "1"},
        {"VulnerabilityID": "CVE-2026-2", "Severity": "CRITICAL", "FixedVersion": None, "PkgName": "q", "InstalledVersion": "1"},
    ]}]}
    path = tmp_path / "report.json"
    path.write_text(json.dumps(report))
    kev = tmp_path / "kev.json"
    kev.write_text(json.dumps({"vulnerabilities": []}))
    assert scan.judge([path], kev, tmp_path / "none.yaml", TODAY) == [], "a HIGH nobody exploits and a CRITICAL with no fix wait"
    kev.write_text(json.dumps({"vulnerabilities": [{"cveID": "CVE-2026-1"}]}))
    assert scan.judge([path], kev, tmp_path / "none.yaml", TODAY) == [
        "example.org/x: CVE-2026-1 HIGH in p 1, fixed in 2"
    ]


def test_an_excuse_needs_a_reason_a_task_and_an_expiry_and_lapses(tmp_path):
    ignores = tmp_path / "ignore.yaml"
    entry = "- {image: docker.gitea.com/gitea, cve: CVE-2026-60004, reason: upgrade scheduled, task: T-3286, expires: %s}\n"
    ignores.write_text(entry % "2026-10-14")
    found = scan.judge([FIXTURE], None, ignores, TODAY)
    assert not any("CVE-2026-60004" in line for line in found), found
    ignores.write_text(entry % "2026-10-01")
    assert any("expired on 2026-10-01" in line for line in scan.judge([FIXTURE], None, ignores, TODAY))
    ignores.write_text("- {image: docker.gitea.com/gitea, cve: CVE-2026-60004, expires: 2026-10-14}\n")
    found = scan.judge([FIXTURE], None, ignores, TODAY)
    assert any("an excuse names its reason, a task" in line for line in found)
    assert any("CVE-2026-60004 CRITICAL" in line for line in found), "an incomplete excuse excuses nothing"


def _report(tmp_path, name, artifact_type, artifact, vulnerabilities):
    path = tmp_path / name
    path.write_text(json.dumps({"ArtifactName": artifact, "ArtifactType": artifact_type,
                                "Results": [{"Vulnerabilities": vulnerabilities}]}))
    return path


def test_summary_turns_each_fixable_high_or_critical_into_one_board_key(tmp_path):
    # T-3834 (d): the summary is what tasks/file-failures reads; one key per (image, CVE) of an
    # image and per (crate, CVE) of a lockfile, so a CVE in two packages of one image is one task.
    image = _report(tmp_path, "image.json", "container_image", "example.org/x@sha256:1", [
        {"VulnerabilityID": "CVE-2026-1", "Severity": "HIGH", "FixedVersion": "2", "PkgName": "openssl", "InstalledVersion": "1"},
        {"VulnerabilityID": "CVE-2026-1", "Severity": "HIGH", "FixedVersion": "2", "PkgName": "libssl", "InstalledVersion": "1"},
        {"VulnerabilityID": "CVE-2026-2", "Severity": "CRITICAL", "FixedVersion": "", "PkgName": "q", "InstalledVersion": "1"},
        {"VulnerabilityID": "CVE-2026-3", "Severity": "MEDIUM", "FixedVersion": "2", "PkgName": "r", "InstalledVersion": "1"},
    ])
    lock = _report(tmp_path, "fs.json", "filesystem", ".", [
        {"VulnerabilityID": "RUSTSEC-2026-0001", "Severity": "CRITICAL", "FixedVersion": ">=0.5.1", "PkgName": "rustls", "InstalledVersion": "0.5.0"},
    ])
    summary = scan.summary([image, lock], "joinedcontext-platform", "run-1", tmp_path / "none.yaml", None, TODAY)
    assert summary["check"] == "vuln" and summary["repo"] == "joinedcontext-platform" and summary["run"] == "run-1"
    failing = {r["key"]: r for r in summary["results"] if r["verdict"] == "fail"}
    assert set(failing) == {"example.org/x/CVE-2026-1", "rustls/RUSTSEC-2026-0001"}, failing
    assert "openssl" in failing["example.org/x/CVE-2026-1"]["detail"] and "libssl" in failing["example.org/x/CVE-2026-1"]["detail"]
    assert failing["rustls/RUSTSEC-2026-0001"]["title"] == "rustls: RUSTSEC-2026-0001 CRITICAL, fixed in >=0.5.1"


def test_summary_skips_an_excused_finding_and_passes_a_fixed_one_of_a_scanned_target(tmp_path):
    ignores = tmp_path / "ignore.yaml"
    ignores.write_text("- {image: example.org/x, cve: CVE-2026-1, reason: upgrade scheduled, task: T-3286, expires: 2026-10-14}\n")
    image = _report(tmp_path, "image.json", "container_image", "example.org/x@sha256:2", [
        {"VulnerabilityID": "CVE-2026-1", "Severity": "HIGH", "FixedVersion": "2", "PkgName": "p", "InstalledVersion": "1"},
    ])
    board = tmp_path / "board"
    board.mkdir()
    (board / "T-9001-vuln-example-org-x-cve-2026-9.md").write_text("---\nid: T-9001\nstatus: todo\ncheck: vuln/example.org/x/CVE-2026-9\n---\n")
    (board / "T-9002-vuln-other-cve-2026-8.md").write_text("---\nid: T-9002\nstatus: todo\ncheck: vuln/example.org/other/CVE-2026-8\n---\n")
    summary = scan.summary([image], "joinedcontext-deployment", "run-2", ignores, board, TODAY)
    verdicts = {r["key"]: r["verdict"] for r in summary["results"]}
    assert verdicts == {"example.org/x/CVE-2026-9": "pass"}, "excused waits; a CVE gone from a scanned image closes; an unscanned image says nothing"


def test_summary_command_prints_the_json_file_failures_reads(tmp_path, capsys):
    image = _report(tmp_path, "image.json", "container_image", "example.org/x@sha256:1", [])
    assert scan.main(["scan-pinned-images.py", "summary", str(image), "--repo", "joinedcontext-portal", "--run", "r"]) == 0
    assert json.loads(capsys.readouterr().out) == {"check": "vuln", "repo": "joinedcontext-portal", "run": "r",
                                                   "requirements": [], "results": []}
