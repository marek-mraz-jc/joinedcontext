#!/usr/bin/env python3
"""Every image the deployment pins, and what a vulnerability scan of them decides (T-3289).

    scan-pinned-images.py list                         one reference per line, registry/repository@digest
    scan-pinned-images.py judge REPORT... [--kev KEV]  exit 1 when a finding blocks, naming each
    scan-pinned-images.py summary REPORT... --repo R --run ID [--board DIR]
                                                       the findings as a tasks/file-failures summary

`list` reads every `components/*/images.yaml`: an entry with `repository`, `tag` and `digest`
(and `registry` when the chart joins it) is one pinned image, scanned by its digest.

`judge` reads trivy's JSON reports (`trivy image --format json`). A finding blocks when a fixed
version exists and it is CRITICAL, or HIGH and listed in CISA's Known Exploited Vulnerabilities
catalogue (`--kev`, its JSON feed). `ci/pinned-images-ignore.yaml` may excuse one, and only with
a reason, a task id and an expiry date; an entry that lacks one, or has expired, excuses nothing.
"""

import datetime
import json
import pathlib
import re
import sys

import yaml

ROOT = pathlib.Path(__file__).resolve().parent.parent
IGNORES = ROOT / "ci" / "pinned-images-ignore.yaml"
TASK = re.compile(r"^T-\d{4}$")


def pinned(root: pathlib.Path = ROOT) -> list[str]:
    found: set[str] = set()

    def walk(node: object) -> None:
        if isinstance(node, dict):
            if "repository" in node and "digest" in node:
                repository = str(node["repository"])
                if node.get("registry"):
                    repository = f"{node['registry']}/{repository}"
                found.add(f"{repository}@{node['digest']}")
                return
            for value in node.values():
                walk(value)
        elif isinstance(node, list):
            for value in node:
                walk(value)

    for path in sorted(root.glob("components/*/images.yaml")):
        for document in yaml.safe_load_all(path.read_text()):
            walk(document)
    return sorted(found)


def ignores(path: pathlib.Path, today: datetime.date) -> tuple[set[tuple[str, str]], list[str]]:
    """The (image repository, CVE) pairs excused today, and what is wrong with the file."""
    if not path.exists():
        return set(), []
    excused, problems = set(), []
    for entry in yaml.safe_load(path.read_text()) or []:
        cve, image = entry.get("cve"), entry.get("image")
        reason, task, expires = entry.get("reason"), str(entry.get("task", "")), entry.get("expires")
        if not (cve and image and reason and TASK.match(task) and isinstance(expires, datetime.date)):
            problems.append(f"{cve or '?'} on {image or '?'}: an excuse names its reason, a task (T-nnnn) and an expiry date")
            continue
        if expires < today:
            problems.append(f"{cve} on {image}: the excuse expired on {expires} ({task})")
            continue
        excused.add((image, cve))
    return excused, problems


def blocking(report: dict, kev: set[str], excused: set[tuple[str, str]]) -> list[str]:
    image = str(report.get("ArtifactName", "?")).split("@")[0]
    found = []
    for result in report.get("Results") or []:
        for vulnerability in result.get("Vulnerabilities") or []:
            cve = vulnerability.get("VulnerabilityID", "")
            severity = vulnerability.get("Severity", "")
            fixed = vulnerability.get("FixedVersion")
            if not fixed or (image, cve) in excused:
                continue
            if severity == "CRITICAL" or (severity == "HIGH" and cve in kev):
                line = (
                    f"{image}: {cve} {severity} in {vulnerability.get('PkgName', '?')} "
                    f"{vulnerability.get('InstalledVersion', '?')}, fixed in {fixed}"
                )
                # One finding per package, however many targets of the image carry it.
                if line not in found:
                    found.append(line)
    return found


def judge(reports: list[pathlib.Path], kev_path: pathlib.Path | None, ignore_path: pathlib.Path = IGNORES,
          today: datetime.date | None = None) -> list[str]:
    kev = set()
    if kev_path is not None:
        kev = {entry["cveID"] for entry in json.loads(kev_path.read_text()).get("vulnerabilities", [])}
    excused, problems = ignores(ignore_path, today or datetime.date.today())
    found = list(problems)
    for path in reports:
        found += blocking(json.loads(path.read_text()), kev, excused)
    return found


def _open_vuln_tasks(board: pathlib.Path) -> list[tuple[str, str]]:
    """(key, repo) of every open task on the board that a `vuln` summary filed."""
    found = []
    for task in board.glob("T-*.md"):
        head = task.read_text(encoding="utf-8", errors="replace").split("\n---", 1)[0]
        if re.search(r"^status:\s*done\b", head, re.M):
            continue
        repo = re.search(r"^repo:\s*(\S+)", head, re.M)
        for match in re.finditer(r"^check:\s*vuln/(.+?)\s*$", head, re.M):
            found.append((match.group(1), repo.group(1) if repo else ""))
    return found


def summary(reports: list[pathlib.Path], repo: str, run: str, ignore_path: pathlib.Path = IGNORES,
            board: pathlib.Path | None = None, today: datetime.date | None = None) -> dict:
    """T-3834 (d): every fixable HIGH or CRITICAL as one `tasks/file-failures` key, `{target}/{id}`.

    The target of an image is its repository; of a lockfile (trivy fs) it is the package, so one
    crate bump closes it. With `--board`, an open task whose key this run no longer finds passes,
    for an image this run scanned or for a lockfile package of the same repository.
    """
    excused, _ = ignores(ignore_path, today or datetime.date.today())
    findings: dict[str, dict] = {}
    present, images, lockfiles = set(), set(), False
    for path in reports:
        report = json.loads(path.read_text())
        image = report.get("ArtifactType", "container_image") == "container_image"  # trivy fs says "filesystem"
        artifact = str(report.get("ArtifactName", "?")).split("@")[0]
        if image:
            images.add(artifact)
        else:
            lockfiles = True
        for result in report.get("Results") or []:
            for vulnerability in result.get("Vulnerabilities") or []:
                cve, severity = vulnerability.get("VulnerabilityID", ""), vulnerability.get("Severity", "")
                fixed, package = vulnerability.get("FixedVersion"), vulnerability.get("PkgName", "?")
                if not fixed or severity not in ("HIGH", "CRITICAL"):
                    continue
                target = artifact if image else package
                key = f"{target}/{cve}"
                present.add(key)
                if (target, cve) in excused:
                    continue
                line = f"{package} {vulnerability.get('InstalledVersion', '?')}, fixed in {fixed}"
                entry = findings.setdefault(key, {"key": key, "verdict": "fail", "repo": repo,
                                                  "title": f"{target}: {cve} {severity}, fixed in {fixed}", "detail": ""})
                if line not in entry["detail"]:
                    entry["detail"] += line + "\n"
    results = list(findings.values())
    for key, task_repo in _open_vuln_tasks(board) if board else []:
        target = key.rsplit("/", 1)[0]
        scanned = target in images or (lockfiles and "/" not in target and task_repo == repo)
        if scanned and key not in present:
            results.append({"key": key, "verdict": "pass"})
    return {"check": "vuln", "repo": repo, "run": run, "requirements": [], "results": results}


def _option(args: list[str], name: str) -> tuple[str | None, list[str]]:
    if name not in args or args.index(name) + 1 >= len(args):
        return None, args
    at = args.index(name)
    return args[at + 1], args[:at] + args[at + 2:]


def main(argv: list[str]) -> int:
    if argv[1:2] == ["list"]:
        print("\n".join(pinned()))
        return 0
    if argv[1:2] == ["judge"]:
        args = argv[2:]
        kev = None
        if "--kev" in args:
            at = args.index("--kev")
            kev = pathlib.Path(args[at + 1])
            args = args[:at] + args[at + 2:]
        found = judge([pathlib.Path(a) for a in args], kev)
        for line in found:
            print(line)
        print(f"{len(found)} blocking finding(s) in {len(args)} image(s)")
        return 1 if found else 0
    if argv[1:2] == ["summary"]:
        repo, args = _option(argv[2:], "--repo")
        run, args = _option(args, "--run")
        board, args = _option(args, "--board")
        if not repo or not run or not args:
            print(__doc__, file=sys.stderr)
            return 2
        result = summary([pathlib.Path(a) for a in args], repo, run, board=pathlib.Path(board) if board else None)
        print(json.dumps(result, indent=1))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
