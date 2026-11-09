"""The rehearsal's Apps (apps.sh calls it; owner jc-apps-export).

    apps.py person  --realm-file <state/realms/R.json> --files <export>/gitea/files --password-file F
    apps.py publish --base http://localhost:P --export <export> --token-file <shako token> [--report F]

person   adds rehearsal-app-roles to the realm file kaisatsu imports: a member of every group the
         `spec.access` of a published `visibility: roles` App names, with a password from a file
         (`$(file:...)`, never the value). rehearsal-tester stays the person holding no role.
publish  publishes each prod build the export carries (<export>/apps/<name>/bundle.tar.gz, read only
         from prod by prod-deployer) as shako's generic package `app-<name>`, version
         `<commit>-<digest12>` of the App's `status.build`, under the applications' owner (jc-apps),
         as the build lane published it on prod. The bytes go as they are: jc's Portal checks them
         against status.build when it fetches them (crates/portal/src/apps/fetch.rs) and serves
         nothing that does not match. A version already there answers 409 and is left alone.
         Only to a loopback base; the token is read from its file and never printed."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

PERSON = "rehearsal-app-roles"
APPS_OWNER = "jc-apps"


def app_manifests(files: Path):
    """(project, App document) of every App manifest under the export's repository files."""
    import yaml  # the CI venv's PyYAML

    for f in sorted(files.glob("*/*/**/app.yaml")):
        try:
            docs = list(yaml.safe_load_all(f.read_text()))
        except yaml.YAMLError:
            continue
        for d in docs:
            if isinstance(d, dict) and d.get("kind") == "App":
                yield str((d.get("metadata") or {}).get("namespace") or f.relative_to(files).parts[1]), d


def role_groups(files: Path) -> list[str]:
    """The group names the access of every published roles App names."""
    out = set()
    for _, d in app_manifests(files):
        spec = d.get("spec") or {}
        if spec.get("visibility") != "roles" or spec.get("lifecycle") != "published":
            continue
        for grant in spec.get("access") or []:
            out.update(s["group"] for s in grant.get("subjects") or [] if s.get("group"))
    return sorted(out)


def group_paths(realm: dict) -> dict[str, str]:
    """name -> path of every group of the realm (subgroups too)."""
    out, todo = {}, list(realm.get("groups") or [])
    while todo:
        g = todo.pop()
        out.setdefault(g["name"], g.get("path") or "/" + g["name"])
        todo += g.get("subGroups") or []
    return out


def with_person(realm: dict, groups: list[str], password_file: Path) -> tuple[dict, list[str]]:
    """The realm with PERSON (made again, never twice) in each of `groups` the realm holds, and the
    names it does not hold."""
    paths = group_paths(realm)
    missing = [g for g in groups if g not in paths]
    person = {"username": PERSON, "enabled": True, "email": f"{PERSON}@rehearsal.invalid", "emailVerified": True,
              "firstName": "Rehearsal", "lastName": "App roles",
              "groups": sorted(paths[g] for g in groups if g in paths),
              "credentials": [{"type": "password", "value": f"$(file:{password_file})", "temporary": False}]}
    out = dict(realm)
    out["users"] = [u for u in realm.get("users") or [] if u.get("username") != PERSON] + [person]
    return out, missing


def cmd_person(a) -> int:
    path = Path(a.realm_file)
    realm = json.loads(path.read_text())
    groups = role_groups(Path(a.files))
    realm, missing = with_person(realm, groups, Path(a.password_file))
    fd = os.open(path, os.O_WRONLY | os.O_TRUNC)  # the file keeps its 0600
    with os.fdopen(fd, "w") as f:
        f.write(json.dumps(realm, indent=1))
    print(f"apps.py: {PERSON} in {len(groups) - len(missing)} groups of realm {realm.get('realm')}"
          + (f"; not in the realm: {missing}" if missing else ""))
    return 0


def loopback_only(base: str) -> None:
    host = urllib.parse.urlparse(base).hostname or ""
    if host not in ("localhost", "127.0.0.1", "::1"):
        raise SystemExit(f"apps.py: {base} is not a loopback address; the rehearsal writes nowhere else")


def builds(export: Path) -> list[dict]:
    """Each App of the export whose status.build names a build and whose bundle the export holds:
    {project, name, commit, digest, version, file}."""
    out = []
    for project, d in app_manifests(export / "gitea" / "files"):
        name = str((d.get("metadata") or {}).get("name"))
        build = (d.get("status") or {}).get("build") or {}
        bundle = export / "apps" / name / "bundle.tar.gz"
        if not (build.get("commit") and build.get("digest") and bundle.is_file()):
            continue
        hexd = build["digest"].removeprefix("sha256:")
        out.append({"project": project, "name": name, "commit": build["commit"], "digest": build["digest"],
                    "version": f"{build['commit']}-{hexd[:12]}", "file": bundle})
    return out


def post(base: str, token: str, path: str, body: dict, timeout: float = 120) -> tuple[int, str]:
    req = urllib.request.Request(base.rstrip("/") + path, data=json.dumps(body).encode(), method="POST",
                                 headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read().decode(errors="replace")[:300]
    except urllib.error.HTTPError as e:
        return e.code, (e.read() or b"").decode(errors="replace")[:300]
    except (urllib.error.URLError, OSError) as e:
        return 0, str(e)


def wait_for(base: str, seconds: float) -> bool:
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            with urllib.request.urlopen(base.rstrip("/") + "/git/odata", timeout=5):
                return True
        except urllib.error.HTTPError:
            return True  # shako answers, if only with a refusal
        except (urllib.error.URLError, OSError):
            time.sleep(2)
    return False


def cmd_publish(a) -> int:
    loopback_only(a.base)
    token = Path(a.token_file).read_text().strip()
    todo = builds(Path(a.export))
    if not wait_for(a.base, a.wait):
        print(f"apps.py: {a.base} did not answer within {a.wait} s", file=sys.stderr)
        return 1
    report, failed = [], 0
    for b in todo:
        data = b["file"].read_bytes()
        found = "sha256:" + hashlib.sha256(data).hexdigest()
        st, text = post(a.base, token, "/git/odata/PublishPackage", {
            "Owner": a.owner, "Package": f"app-{b['name']}", "Version": b["version"], "File": "bundle.tar.gz",
            "ContentBase64": base64.b64encode(data).decode()})
        result = "published" if st in (200, 201) else "unchanged" if st == 409 else "error"
        failed += result == "error"
        report.append({"app": b["name"], "package": f"{a.owner}/app-{b['name']}/{b['version']}", "bytes": len(data),
                       "sha256": found, "statusBuild": b["digest"], "matches": found == b["digest"],
                       "result": result, **({"answer": f"{st} {text}"} if result == "error" else {})})
        print(f"apps.py: {b['name']} {result}" + ("" if found == b["digest"] else
                                                   " (bytes differ from status.build: the Portal will refuse them)"))
    if a.report:
        Path(a.report).write_text(json.dumps(report, indent=1))
    return 1 if failed else 0


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("person")
    s.add_argument("--realm-file", required=True)
    s.add_argument("--files", required=True)
    s.add_argument("--password-file", required=True)
    s = sub.add_parser("publish")
    s.add_argument("--base", required=True)
    s.add_argument("--export", required=True)
    s.add_argument("--token-file", required=True)
    s.add_argument("--owner", default=APPS_OWNER)
    s.add_argument("--report")
    s.add_argument("--wait", type=float, default=600)
    a = p.parse_args(argv)
    return cmd_person(a) if a.cmd == "person" else cmd_publish(a)


if __name__ == "__main__":
    sys.exit(main())
