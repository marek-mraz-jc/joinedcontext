#!/usr/bin/env python3
"""A `curl` the forge bootstrap Job's script can be run against (T-2392).

The Job is a shell script that talks to exactly two addresses — the forge and the API server —
and it talks to both with `curl`. Putting a `curl` of our own on PATH therefore runs the real
script, in the real order, with no Gitea and no cluster: the contents endpoints keep their files
in a JSON state file, every other endpoint answers what the script's branches look at, and each
call is appended to a log a test can read.

Only what the script asks for is implemented. An address it never calls answers 404, which is
what an honest stub does with a request it does not understand.

    curl [-sS] [-f] [-o FILE] [-w '%{http_code}'] [-X METHOD] [-u U:P] [-H H] [-d BODY]
         [--cacert FILE] URL

The state file (`JC_FAKE_FORGE_STATE`) holds `{"contents": {path: text}, "calls": [...]}`, and
`writes`: every call that is not a read, with who made it (`basic:<user>` or `token:<token>`);
the log is `JC_FAKE_FORGE_CALLS`, one `METHOD URL` per line. The forge's users, collaborators,
teams, tokens and branch rules are kept in the state too (PF-105, PF-106).
"""

from __future__ import annotations

import base64
import json
import os
import re
import sys
from urllib.parse import unquote

STATE = os.environ["JC_FAKE_FORGE_STATE"]
CALLS = os.environ["JC_FAKE_FORGE_CALLS"]
ORG = os.environ.get("ORG", "joinedcontext")
REPO = os.environ.get("REPO", "configuration")


def load() -> dict:
    with open(STATE, encoding="utf-8") as handle:
        return json.load(handle)


def save(state: dict) -> None:
    with open(STATE, "w", encoding="utf-8") as handle:
        json.dump(state, handle, indent=1, sort_keys=True)


def blob(path: str, text: str) -> dict:
    """What Gitea answers for one file: the content base64-encoded and a 40-hex sha."""
    import hashlib

    return {
        "name": path.rsplit("/", 1)[-1],
        "path": path,
        "sha": hashlib.sha1(text.encode("utf-8")).hexdigest(),
        "content": base64.b64encode(text.encode("utf-8")).decode("ascii"),
    }


def git_blob(text: str) -> str:
    """The id `git hash-object` gives the file: sha1 over `blob <size>\\0` and the bytes."""
    import hashlib

    data = text.encode("utf-8")
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()


def head_of(files: dict[str, str]) -> str:
    import hashlib

    return hashlib.sha1(json.dumps(files, sort_keys=True).encode("utf-8")).hexdigest()


def app_repository(path: str, method: str, call: dict, state: dict) -> tuple[int, str] | None:
    """An application's own repository (T-2599): created in the organization, read as a tree,
    written as one commit of file operations, and its main branch's head."""
    repos = state.setdefault("repos", {})
    if re.fullmatch(rf"/api/v1/orgs/{re.escape(ORG)}/repos", path) and method == "POST":
        body = json.loads(call["data"])
        if body["name"] in repos:
            return 409, '{"message":"the repository already exists"}'
        files = {"README.md": f"# {body['name']}\n"} if body.get("auto_init") else {}
        repos[body["name"]] = {"files": files, "private": body.get("private"), "head": head_of(files)}
        return 201, json.dumps({"name": body["name"]})
    match = re.fullmatch(rf"/api/v1/repos/{re.escape(ORG)}/([^/?]+)/(git/trees/main|contents|branches/main)(\?.*)?", path)
    if not match or (match.group(1) != REPO and match.group(1) not in repos):
        return None
    # The configuration repository keeps its files in `contents` (the seed's tree and commit,
    # T-4709), any other one in `repos[name].files`.
    repo = {"files": state["contents"]} if match.group(1) == REPO else repos[match.group(1)]
    what = match.group(2)
    if what == "git/trees/main" and method == "GET":
        tree = [
            {"path": p, "mode": "100644", "type": "blob", "size": len(t.encode("utf-8")), "sha": git_blob(t), "url": f"http://forge.test/{p}"}
            for p, t in sorted(repo["files"].items())
        ]
        return 200, json.dumps({"sha": "c" * 40, "url": "http://forge.test/tree", "tree": tree, "truncated": False, "page": 1, "total_count": len(tree)}, separators=(",", ":"))
    if what == "branches/main" and method == "GET":
        return 200, json.dumps({"name": "main", "commit": {"id": repo["head"], "message": "seed"}}, separators=(",", ":"))
    if what == "contents" and method == "POST":
        body = json.loads(call["data"])
        files = dict(repo["files"])
        for change in body["files"]:
            held = files.get(change["path"])
            if change["operation"] == "create":
                if held is not None:
                    return 422, '{"message":"the file already exists"}'
            elif held is None or change.get("sha") != git_blob(held):
                return 409, '{"message":"sha does not match"}'
            if change["operation"] == "delete":
                del files[change["path"]]
            else:
                files[change["path"]] = base64.b64decode(change["content"]).decode("utf-8")
        repo["files"] = files
        repo["head"] = head_of(files)
        repo.setdefault("commits", []).append(body["message"])
        if match.group(1) == REPO:
            state["contents"] = files
            state.setdefault("commits", []).append(body["message"])
        return 201, json.dumps({"commit": {"sha": repo["head"]}})
    return None


def team_by_id(state: dict, team_id: str) -> dict | None:
    return next((t for t in state.setdefault("teams", {}).values() if str(t["id"]) == team_id), None)


def teams(path: str, method: str, call: dict, state: dict) -> tuple[int, str] | None:
    """An organization's teams, keyed `org/name` in `state["teams"]`, each with its id, what it
    was created with and its members. Every organization has its Owners team (T-3030)."""
    held = state.setdefault("teams", {})

    def make(org: str, name: str, spec: dict) -> dict:
        team = {"id": 10 + len(held), "spec": spec, "members": []}
        held[f"{org}/{name}"] = team
        return team

    search = re.fullmatch(r"/api/v1/orgs/([^/]+)/teams/search\?q=(.+)", path)
    if search and method == "GET":
        org, name = search.group(1), unquote(search.group(2))
        if name == "Owners" and f"{org}/Owners" not in held:
            make(org, "Owners", {"permission": "owner"})
        team = held.get(f"{org}/{name}")
        return 200, json.dumps({"data": [{"id": team["id"], "name": name}] if team else [], "ok": True})
    create = re.fullmatch(r"/api/v1/orgs/([^/]+)/teams", path)
    if create and method == "POST":
        body = json.loads(call["data"])
        if f"{create.group(1)}/{body['name']}" in held:
            return 422, '{"message":"team already exists"}'
        team = make(create.group(1), body["name"], body)
        return 201, json.dumps({"id": team["id"], "name": body["name"]})
    listing = re.fullmatch(r"/api/v1/teams/(\d+)/members\?limit=50&page=(\d+)", path)
    if listing and method == "GET":
        team = team_by_id(state, listing.group(1))
        if team is None:
            return 404, "{}"
        page = team["members"] if listing.group(2) == "1" else []
        return 200, json.dumps([{"id": i, "login": login, "login_name": ""} for i, login in enumerate(page)])
    member = re.fullmatch(r"/api/v1/teams/(\d+)/members/([^/]+)", path)
    if member and method in ("PUT", "DELETE"):
        team = team_by_id(state, member.group(1))
        login = member.group(2)
        if team is not None and method == "PUT" and login not in team["members"]:
            team["members"].append(login)
        if team is not None and method == "DELETE" and login in team["members"]:
            team["members"].remove(login)
    return None


def identities(path: str, method: str, call: dict, state: dict) -> tuple[int, str] | None:
    """Users, collaborators, the Portal's team, the organization's repositories and the branch
    rule: what the Job converges before the seed (PF-105, PF-106)."""
    users = state.setdefault("users", {})
    user = re.fullmatch(r"/api/v1/users/([^/?]+)", path)
    if user and method == "GET":
        held = users.get(user.group(1))
        return (200, json.dumps({"login": user.group(1), **held})) if held else (404, "{}")
    if path == "/api/v1/admin/users" and method == "POST":
        body = json.loads(call["data"])
        users[body["username"]] = {"is_admin": False, "email": body["email"], "password_set": bool(body["password"])}
        return 201, json.dumps({"login": body["username"]})
    edit = re.fullmatch(r"/api/v1/admin/users/([^/?]+)", path)
    if edit and method == "PATCH":
        body = json.loads(call["data"])
        users.setdefault(edit.group(1), {}).update({"patched": body})
        return 200, "{}"
    collaborator = re.fullmatch(r"/api/v1/repos/[^/]+/([^/]+)/collaborators/([^/]+)", path)
    if collaborator and method == "PUT":
        state.setdefault("collaborators", {})[f"{collaborator.group(1)}/{collaborator.group(2)}"] = json.loads(call["data"])["permission"]
        return 204, ""
    team = re.fullmatch(r"/api/v1/teams/(\d+)", path)
    if team and method == "PATCH":
        body = json.loads(call["data"])
        state.setdefault("team_units", {})[body["name"]] = body["units_map"]
        return 200, "{}"
    member = re.fullmatch(r"/api/v1/teams/(\d+)/members/([^/]+)", path)
    if member and method == "PUT":
        state.setdefault("members", []).append(member.group(2))
        state.setdefault("team_members", {}).setdefault(member.group(1), []).append(member.group(2))
        return 204, ""
    if member and method == "DELETE":
        state.setdefault("left", []).append(member.group(2))
        return 204, ""
    moved = state.setdefault("moved", {})
    transfer = re.fullmatch(rf"/api/v1/repos/{re.escape(ORG)}/([^/]+)/transfer", path)
    if transfer and method == "POST":
        name = transfer.group(1)
        if name not in state.setdefault("repos", {}):
            return 404, "{}"
        moved[name] = json.loads(call["data"])["new_owner"]
        del state["repos"][name]
        return 202, "{}"
    # The runs a repository had waiting when it moved (T-3026): `queued` is {name: [branch, ...]},
    # one entry per run, and every dispatch is recorded as `owner/name@ref`.
    runs = re.fullmatch(r"/api/v1/repos/([^/]+)/([^/]+)/actions/runs\?status=queued&limit=50", path)
    if runs and method == "GET":
        branches = state.setdefault("queued", {}).get(runs.group(2), [])
        listed = [{"id": i + 1, "head_branch": b, "status": "queued"} for i, b in enumerate(branches)]
        return 200, json.dumps({"total_count": len(listed), "workflow_runs": listed}, separators=(",", ":"))
    # The newest run of a repository (T-5774): `runs` is {name: {"head_sha", "conclusion"}}, in
    # Gitea 1.27's field order, the conclusion after the nested repository it carries.
    newest = re.fullmatch(r"/api/v1/repos/([^/]+)/([^/]+)/actions/runs\?limit=1", path)
    if newest and method == "GET":
        run = state.setdefault("runs", {}).get(newest.group(2))
        listed = [] if run is None else [{
            "id": 1, "head_sha": run["head_sha"], "head_branch": "main", "status": run.get("status", "completed"),
            "repository": {"id": 7, "name": newest.group(2), "default_branch": "main"},
            "conclusion": run["conclusion"],
        }]
        return 200, json.dumps({"total_count": len(listed), "workflow_runs": listed})
    dispatch = re.fullmatch(r"/api/v1/repos/([^/]+)/([^/]+)/actions/workflows/build\.yml/dispatches", path)
    if dispatch and method == "POST":
        ref = json.loads(call["data"])["ref"]
        state.setdefault("dispatched", []).append(f"{dispatch.group(1)}/{dispatch.group(2)}@{ref}")
        return 204, ""
    listing = re.fullmatch(r"/api/v1/orgs/([^/]+)/repos\?limit=50&page=(\d+)", path)
    if listing and method == "GET":
        owner = listing.group(1)
        if owner == ORG:
            names = [REPO, *sorted(state.setdefault("repos", {}))]
        else:
            names = sorted(name for name, to in moved.items() if to == owner)
        page = int(listing.group(2))
        return 200, json.dumps([{"id": i, "name": n} for i, n in enumerate(names)] if page == 1 else [], separators=(",", ":"))
    rules = state.setdefault("rules", {})
    rule = re.fullmatch(r"/api/v1/repos/[^/]+/([^/]+)/branch_protections(?:/([^/]+))?", path)
    if rule:
        key = f"{rule.group(1)}/{rule.group(2)}"
        if method == "GET":
            return (200, json.dumps(rules[key])) if key in rules else (404, "{}")
        body = json.loads(call["data"])
        if method == "POST":
            rules[f"{rule.group(1)}/{body['rule_name']}"] = body
            return 201, json.dumps(body)
        if method == "PATCH" and key in rules:
            rules[key].update(body)
            return 200, json.dumps(rules[key])
    if path.startswith("/apis/apps/v1/namespaces/") and method == "PATCH":
        state.setdefault("restarted", []).append(path.split("/namespaces/", 1)[1])
        return (200, "{}") if state.get("deployments_exist") else (404, "{}")
    return None


def parse(argv: list[str]) -> dict:
    out = {"method": None, "out": None, "code": False, "data": None, "fail": False, "url": None, "who": None}
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg in ("-s", "-S", "-sS", "-k"):
            pass
        elif arg == "-f":
            out["fail"] = True
        elif arg in ("-o", "-u", "-H", "--cacert"):
            value = argv[index + 1]
            if arg == "-o":
                out["out"] = value
            elif arg == "-u":
                out["who"] = "basic:" + value.split(":", 1)[0]
            elif arg == "-H" and value.lower().startswith("authorization: token "):
                out["who"] = "token:" + value.split(" ", 2)[2]
            index += 1
        elif arg == "-w":
            out["code"] = "%{http_code}" in argv[index + 1]
            index += 1
        elif arg == "-X":
            out["method"] = argv[index + 1]
            index += 1
        elif arg == "-d":
            data = argv[index + 1]
            # `-d @file` sends the file, which is how the Job posts a tree too big for argv.
            if data.startswith("@"):
                with open(data[1:], encoding="utf-8") as handle:
                    data = handle.read()
            out["data"] = data
            index += 1
        elif arg.startswith("-"):
            pass
        else:
            out["url"] = arg
        index += 1
    if out["method"] is None:
        out["method"] = "POST" if out["data"] is not None else "GET"
    return out


def answer(call: dict, state: dict) -> tuple[int, str]:
    url, method = call["url"], call["method"]
    path = url.split("://", 1)[-1].split("/", 1)[-1]
    path = "/" + path

    if path.startswith("/api/healthz"):
        return 200, "{}"
    # The API server: every Secret write is accepted and remembered, every read answers what
    # was written. `read_secret` passes `-f`, so a miss has to be a failure and not an empty
    # body it would read as "there is no token".
    if "/api/v1/namespaces/" in path:
        name = path.rsplit("/", 1)[-1]
        if method == "GET":
            held = state["secrets"].get(name)
            return (200, json.dumps(held)) if held else (404, "{}")
        state["secrets"][name] = json.loads(call["data"])
        return 200, call["data"]
    team = teams(path, method, call, state)
    if team is not None:
        return team
    forge = identities(path, method, call, state)
    if forge is not None:
        return forge
    if re.fullmatch(r"/api/v1/orgs/[^/]+", path):
        return 200, "{}"
    # The organization's packages: what a read:package token may read (AP-108).
    if re.fullmatch(r"/api/v1/packages/[^/?]+", path):
        return 200, "[]"
    if re.fullmatch(r"/api/v1/repos/[^/]+/[^/]+", path):
        name = path.rsplit("/", 1)[-1]
        known = name == REPO or name in state.setdefault("repos", {})
        return (200, '{"default_branch":"main"}') if known else (404, "{}")
    app = app_repository(path, method, call, state)
    if app is not None:
        return app
    if path.endswith("/actions/runners/registration-token") and method == "POST":
        return 200, '{"token":"%s"}' % ("R" * 40)
    tokens = re.fullmatch(r"/api/v1/users/([^/]+)/tokens(?:/([^/]+))?", path)
    if tokens:
        held = state.setdefault("tokens", {})
        owner, name = tokens.group(1), tokens.group(2)
        if method == "POST":
            name = json.loads(call["data"])["name"]
            held[f"{owner}/{name}"] = True
            count = len(state.setdefault("minted", [])) + 1
            state["minted"].append(f"{owner}/{name}")
            return 201, '{"sha1":"%040x"}' % count
        if method == "DELETE":
            return (204, "") if held.pop(f"{owner}/{name}", None) else (404, "{}")

    contents = re.fullmatch(
        rf"/api/v1/repos/{re.escape(ORG)}/([^/?]+)/contents/(.+?)(\?ref=.*)?", path
    )
    # The configuration repository keeps its files in `contents`, any other one (a project
    # repository of layout 2) in `repos[name].files`.
    if contents and contents.group(1) != REPO and contents.group(1) not in state.setdefault("repos", {}):
        return 404, "{}"
    if contents:
        files = state["contents"] if contents.group(1) == REPO else state["repos"][contents.group(1)]["files"]
        file_path = unquote(contents.group(2))
        held = files.get(file_path)
        if method == "GET":
            if held is not None:
                return 200, json.dumps(blob(file_path, held))
            # A folder: Gitea answers the entries directly under it, without their content.
            prefix = file_path.rstrip("/") + "/"
            entries = [
                {**{k: v for k, v in blob(path, text).items() if k != "content"}, "type": "file"}
                for path, text in sorted(files.items())
                if path.startswith(prefix) and "/" not in path[len(prefix):]
            ]
            return (200, json.dumps(entries)) if entries else (404, "{}")
        body = json.loads(call["data"])
        if method == "POST":
            if held is not None:
                return 422, '{"message":"the file already exists"}'
            files[file_path] = base64.b64decode(body["content"]).decode("utf-8")
            return 201, json.dumps(blob(file_path, files[file_path]))
        if method == "PUT":
            if held is None or body.get("sha") != blob(file_path, held)["sha"]:
                return 409, '{"message":"sha does not match"}'
            files[file_path] = base64.b64decode(body["content"]).decode("utf-8")
            return 200, json.dumps(blob(file_path, files[file_path]))
        if method == "DELETE":
            if held is None or body.get("sha") != blob(file_path, held)["sha"]:
                return 409, '{"message":"sha does not match"}'
            del files[file_path]
            return 200, '{"commit":{"sha":"%s"}}' % ("b" * 40)
    return 404, "{}"


def main() -> int:
    call = parse(sys.argv[1:])
    state = load()
    code, body = answer(call, state)
    state.setdefault("calls", []).append(f"{call['method']} {call['url']}")
    if call["method"] != "GET":
        state.setdefault("writes", []).append({"method": call["method"], "url": call["url"], "who": call["who"]})
    save(state)
    with open(CALLS, "a", encoding="utf-8") as handle:
        handle.write(f"{call['method']} {call['url']}\n")

    if call["out"]:
        with open(call["out"], "w", encoding="utf-8") as handle:
            handle.write(body)
    else:
        sys.stdout.write(body)
    if call["code"]:
        sys.stdout.write(str(code))
    # `-f` is what `read_secret` uses to turn a 404 into a failure rather than an empty file.
    return 22 if call["fail"] and code >= 400 else 0


if __name__ == "__main__":
    sys.exit(main())
