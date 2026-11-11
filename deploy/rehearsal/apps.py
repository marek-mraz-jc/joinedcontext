"""The rehearsal's Apps (apps.sh calls it; owner jc-apps-export).

    apps.py person  --realm-file <state/realms/R.json> --files <export>/gitea/files --password-file F
    apps.py publish --base http://localhost:P --export <export> --token-file <shako token> [--report F]
    apps.py grant   --base http://localhost:P --export <export> --token-file <shako token> --realm-file R

person   adds rehearsal-app-roles to the realm file kaisatsu imports: a member of every group a
         `viewer` RoleBinding names (the realm's own groups, which open a `project` or
         `organization` App), with a password from a file (`$(file:...)`, never the value).
         rehearsal-tester stays the person in no group. Its roles come from `grant`.
publish  publishes each prod build the export carries (<export>/apps/<name>/bundle.tar.gz, read only
         from prod by prod-deployer) as shako's generic package `app-<name>`, version
         `<commit>-<digest12>` of the App's `status.build`, under the applications' owner (jc-apps),
         as the build lane published it on prod. The bytes go as they are: jc's Portal checks them
         against status.build when it fetches them (crates/portal/src/apps/fetch.rs) and serves
         nothing that does not match. A version already there answers 409 and is left alone.
         Only to a loopback base; the token is read from its file and never printed.
grant    names rehearsal-app-roles, by its id, a member of each Group manifest those roles Apps'
         access names, in the rehearsal's own configuration repository in shako: on prod a Group
         manifest is the membership, and the Portal takes out of a group whoever it does not
         name. The id is the `sub` of the person's token (admin-cli, the realm's direct grant).
         A rehearsal re-import puts the prod file back; the next up.sh grants again."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rehearsal  # noqa: E402  (merge_change: the kit's one way onto a protected main)

PERSON = "rehearsal-app-roles"
UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
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


def app_projects(files: Path) -> list[str]:
    """The projects of every published App that is not public: their endpoints admit a person by
    the project's own group (PF-87), which a role holder on prod is in."""
    return sorted({project for project, d in app_manifests(files)
                   if (d.get("spec") or {}).get("lifecycle") == "published"
                   and (d.get("spec") or {}).get("visibility") != "public"})


def viewer_groups(files: Path) -> list[str]:
    """The groups the `viewer` RoleBindings of the export's repositories name."""
    import yaml

    out = set()
    for f in sorted(files.glob("*/*/**/*.yaml")):
        try:
            docs = list(yaml.safe_load_all(f.read_text()))
        except yaml.YAMLError:
            continue
        for d in docs:
            spec = d.get("spec") or {} if isinstance(d, dict) else {}
            if isinstance(d, dict) and d.get("kind") == "RoleBinding" and spec.get("role") == "viewer":
                out.update(s["group"] for s in spec.get("subjects") or [] if s.get("group"))
    return sorted(out)


def group_paths(realm: dict) -> dict[str, str]:
    """name -> path of every group of the realm (subgroups too)."""
    out, todo = {}, list(realm.get("groups") or [])
    while todo:
        g = todo.pop()
        out.setdefault(g["name"], g.get("path") or "/" + g["name"])
        todo += g.get("subGroups") or []
    return out


def with_person(realm: dict, groups: list[str], password_file: Path,
                person_id: str | None = None) -> tuple[dict, list[str]]:
    """The realm with PERSON (made again, never twice) in each of `groups` the realm holds, and the
    names it does not hold. `person_id` gives PERSON the id a prod read Policy names (the loopback
    prod rehearsal only, supervisor 2026-11-09: banskabystrica-zaznamy's one reader); the person
    keeps the rehearsal's name, password and no e-mail of the source."""
    if person_id is not None and not UUID.fullmatch(person_id):
        raise SystemExit(f"apps.py: --id {person_id!r} is no user id")
    paths = group_paths(realm)
    missing = [g for g in groups if g not in paths]
    person = {"username": PERSON, "enabled": True, "email": f"{PERSON}@rehearsal.invalid", "emailVerified": True,
              "firstName": "Rehearsal", "lastName": "App roles",
              "groups": sorted(paths[g] for g in groups if g in paths),
              "credentials": [{"type": "password", "value": f"$(file:{password_file.name})", "temporary": False}]}
    if person_id:
        person["id"] = person_id
    out = dict(realm)
    out["users"] = [u for u in realm.get("users") or [] if u.get("username") != PERSON] + [person]
    return out, missing


def cmd_person(a) -> int:
    path = Path(a.realm_file)
    realm = json.loads(path.read_text())
    groups = sorted({*viewer_groups(Path(a.files)), *app_projects(Path(a.files))})
    realm, missing = with_person(realm, groups, Path(a.password_file), a.id)
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


def group_files(files: Path, names: list[str]) -> dict[str, tuple[str, str, str]]:
    """name -> (owner, repo, path in the repo) of each Group manifest of `names` in the export."""
    import yaml

    out = {}
    for f in sorted(files.glob("*/*/**/*.yaml")):
        text = f.read_text(errors="replace")
        if "kind: Group" not in text:
            continue
        try:
            docs = list(yaml.safe_load_all(text))
        except yaml.YAMLError:
            continue
        for d in docs:
            name = isinstance(d, dict) and d.get("kind") == "Group" and (d.get("metadata") or {}).get("name")
            if name in names:
                owner, repo, *rest = f.relative_to(files).parts
                out[name] = (owner, repo, "/".join(rest))
    return out


def with_member(text: str, user: str) -> str:
    """The Group manifest with `- user: <user>` in spec.members (the text otherwise as it was)."""
    import yaml

    doc = yaml.safe_load(text)
    if any(m.get("user") == user for m in (doc.get("spec") or {}).get("members") or []):
        return text
    lines = text.splitlines(keepends=True)
    for i, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("members:"):
            nxt = lines[i + 1] if i + 1 < len(lines) else ""
            # the list's own indent: YAML allows its items level with the key ("indentless")
            pad = nxt[: len(nxt) - len(nxt.lstrip())] if nxt.lstrip().startswith("- ") else \
                " " * (len(line) - len(line.lstrip()) + 2)
            item = f"{pad}- user: {user}\n"
            if stripped == "members:":
                out = "".join(lines[: i + 1] + [item] + lines[i + 1:])
            elif stripped.replace(" ", "") == "members:[]":
                out = "".join(lines[:i] + [line.split("members:")[0] + "members:\n", item] + lines[i + 1:])
            else:
                break
            want = [*((doc.get("spec") or {}).get("members") or []), {"user": user}]
            try:
                if (yaml.safe_load(out).get("spec") or {}).get("members") == want:
                    return out
            except yaml.YAMLError:
                pass
            break
    doc.setdefault("spec", {}).setdefault("members", []).append({"user": user})
    return yaml.safe_dump(doc, sort_keys=False)


def call(base: str, method: str, path: str, headers: dict, body=None, form=None, timeout: float = 60):
    data = urllib.parse.urlencode(form).encode() if form else json.dumps(body).encode() if body is not None else None
    headers = {**headers, "Content-Type": "application/x-www-form-urlencoded" if form else "application/json"}
    req = urllib.request.Request(base.rstrip("/") + path, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        return e.code, None
    except (urllib.error.URLError, OSError, ValueError):
        return 0, None


def person_id(base: str, realm: dict, secrets_dir: Path) -> str | None:
    """The `sub` of PERSON's token from the realm's admin-cli (a direct grant, no client secret);
    the password file is named relative to kaisatsu's import secrets dir `secrets_dir`."""
    user = next((u for u in realm.get("users") or [] if u.get("username") == PERSON), {})
    ref = next((c.get("value", "") for c in user.get("credentials") or [] if c.get("type") == "password"), "")
    pw_file = secrets_dir / ref[len("$(file:"):-1] if ref.startswith("$(file:") else None
    if not pw_file or not pw_file.is_file():
        return None
    st, body = call(base, "POST", f"/id/realms/{realm.get('realm')}/protocol/openid-connect/token", {}, form={
        "grant_type": "password", "client_id": "admin-cli", "username": PERSON,
        "password": pw_file.read_text().strip()})
    token = (body or {}).get("access_token") if st == 200 else None
    if not token:
        return None
    claims = token.split(".")[1]
    return json.loads(base64.urlsafe_b64decode(claims + "=" * (-len(claims) % 4))).get("sub")


def cmd_grant(a) -> int:
    loopback_only(a.base)
    if not wait_for(a.base, a.wait):
        print(f"apps.py: {a.base} did not answer within {a.wait} s", file=sys.stderr)
        return 1
    realm = json.loads(Path(a.realm_file).read_text())
    user = person_id(a.base, realm, Path(a.secrets_dir))
    if not user:
        print(f"apps.py: no token for {PERSON} in realm {realm.get('realm')}: nothing granted", file=sys.stderr)
        return 1
    auth = {"Authorization": f"token {Path(a.token_file).read_text().strip()}"}
    files = Path(a.export) / "gitea" / "files"
    failed = 0
    for name, (owner, repo, path) in sorted(group_files(files, role_groups(files)).items()):
        url = f"/git/api/v1/projects/{owner}/repositories/{repo}/contents/{path}"
        st, held = call(a.base, "GET", url, auth)
        if st != 200 or not isinstance(held, dict):
            print(f"apps.py: {owner}/{repo}/{path}: {st}", file=sys.stderr)
            failed += 1
            continue
        text = base64.b64decode(held["content"]).decode()
        new = with_member(text, user)
        if new == text:
            print(f"apps.py: {name}: {PERSON} already a member")
            continue
        # Through a pull request: the imported main takes no direct push (rehearsal.protect_main).
        try:
            rehearsal.merge_change(lambda m, p, b: call(a.base, m, f"/git/api/v1{p}", auth, body=b), owner, repo,
                                   [{"operation": "update", "path": path, "sha": held["sha"],
                                     "content": base64.b64encode(new.encode()).decode()}],
                                   f"rehearsal: {PERSON} in {name} (apps.py grant)")
        except RuntimeError as e:
            print(f"apps.py: {name}: {e}", file=sys.stderr)
            failed += 1
            continue
        print(f"apps.py: {name}: {PERSON} added")
    return 1 if failed else 0


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("person")
    s.add_argument("--realm-file", required=True)
    s.add_argument("--files", required=True)
    s.add_argument("--password-file", required=True)
    s.add_argument("--id", help="the user id a prod read Policy names (loopback prod rehearsal only)")
    s = sub.add_parser("publish")
    s.add_argument("--base", required=True)
    s.add_argument("--export", required=True)
    s.add_argument("--token-file", required=True)
    s.add_argument("--owner", default=APPS_OWNER)
    s.add_argument("--report")
    s.add_argument("--wait", type=float, default=600)
    s = sub.add_parser("grant")
    s.add_argument("--base", required=True)
    s.add_argument("--export", required=True)
    s.add_argument("--token-file", required=True)
    s.add_argument("--realm-file", required=True)
    s.add_argument("--secrets-dir", required=True, help="kaisatsu's import secrets dir (the password file's)")
    s.add_argument("--wait", type=float, default=600)
    a = p.parse_args(argv)
    return {"person": cmd_person, "publish": cmd_publish, "grant": cmd_grant}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
