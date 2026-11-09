"""The test persons the UI walk signs in as, made in the rehearsal's Portal realm through the
kaisatsu admin API (a token of a client holding manage-users, e.g. the Portal's reconciler).

Three personas, each from what the export's configuration grants a group (RoleBindings bind the
demo people by Keycloak id, which a new person cannot take):

- `admin`: member of `/platform-admins`, the installation's administrators;
- `viewer`: member of an Organization's group that a `role: viewer` RoleBinding names with
  `source: provider` (dev: `zilina`, `barcelona`, `bratislava`), so it reads that Organization;
- `outsider`: no group, signed in and granted nothing.

Usernames are `smoke-ui-<persona>`; each password is a fresh random value held in memory only.
A group a binding names but the realm lacks is created (the rehearsal's own realm)."""

from __future__ import annotations

import json
import secrets
import urllib.parse
from pathlib import Path

import yaml
from common import Http, as_json

REALM = "jc"


def _docs(path: Path) -> list[dict]:
    try:
        return [d for d in yaml.safe_load_all(path.read_text()) if isinstance(d, dict)]
    except (yaml.YAMLError, UnicodeDecodeError):
        return []


class Admin:
    def __init__(self, h: Http, token: str, realm: str = REALM):
        self.h, self.token, self.realm = h, token, realm

    def call(self, method, path, body=None):
        st, _, raw = self.h.call(method, f"/id/admin/realms/{self.realm}{path}", self.token, body=body)
        return st, as_json(raw)

    def groups(self) -> dict[str, str]:
        st, tree = self.call("GET", "/groups?briefRepresentation=false&max=1000")
        if st != 200 or not isinstance(tree, list):
            raise RuntimeError(f"GET groups of realm {self.realm}: {st}")
        out, todo = {}, list(tree)
        while todo:
            g = todo.pop()
            out[g.get("path") or "/" + g.get("name", "")] = g["id"]
            todo += g.get("subGroups") or []
        return out

    def ensure_group(self, path: str) -> tuple[str, bool]:
        have = self.groups()
        if path in have:
            return have[path], False
        if path.count("/") != 1:
            raise RuntimeError(f"group {path}: not in realm {self.realm}, and only a top-level group is made")
        st, _ = self.call("POST", "/groups", {"name": path.lstrip("/")})
        if st not in (201, 409):
            raise RuntimeError(f"create group {path}: {st}")
        return self.groups()[path], True

    def person(self, username: str, groups: list[str]) -> str:
        """Creates (or reuses) the person, sets a new random password, joins exactly `groups`."""
        password = secrets.token_urlsafe(18) + "-7aZ"
        st, body = self.call("POST", "/users", {
            "username": username, "enabled": True, "email": f"{username}@smoke.invalid",
            "emailVerified": True, "firstName": "Smoke", "lastName": username})
        if st not in (201, 409):
            raise RuntimeError(f"create {username}: {st} {str(body)[:160]}")
        st, users = self.call("GET", f"/users?username={urllib.parse.quote(username)}&exact=true")
        if st != 200 or not users:
            raise RuntimeError(f"find {username}: {st}")
        uid = users[0]["id"]
        st, _ = self.call("PUT", f"/users/{uid}/reset-password",
                          {"type": "password", "value": password, "temporary": False})
        if st not in (200, 204):
            raise RuntimeError(f"password of {username}: {st}")
        st, held = self.call("GET", f"/users/{uid}/groups")
        for g in held if isinstance(held, list) else []:
            if g.get("path") not in groups:
                self.call("DELETE", f"/users/{uid}/groups/{g['id']}")
        for path in groups:
            gid, _ = self.ensure_group(path)
            st, _ = self.call("PUT", f"/users/{uid}/groups/{gid}")
            if st not in (200, 204):
                raise RuntimeError(f"group {path} of {username}: {st}")
        return password


def viewer_grants(export: Path | None) -> list[dict]:
    """`role: viewer` RoleBindings with a provider group subject and an organization scope."""
    if not export:
        return []
    out = []
    for f in sorted(export.glob("gitea/files/*/*/users/assignments/*.yaml")):
        for doc in _docs(f):
            spec = doc.get("spec") or {}
            org = (spec.get("scope") or {}).get("organization")
            if doc.get("kind") != "RoleBinding" or spec.get("role") != "viewer" or not org:
                continue
            for s in spec.get("subjects") or []:
                if s.get("group") and s.get("source") == "provider":
                    out.append({"binding": doc["metadata"]["name"], "group": "/" + s["group"].lstrip("/"),
                                "organization": org})
    return out


def projects_of(export: Path | None) -> dict[str, str]:
    """project -> organization, from each repository's project.yaml."""
    out = {}
    for f in sorted(export.glob("gitea/files/*/*/project.yaml")) if export else []:
        for doc in _docs(f):
            if doc.get("kind") == "Project":
                out[doc["metadata"]["name"]] = (doc.get("spec") or {}).get("organizationRef", "")
    return out


def plan(export: Path | None, project: str | None) -> list[dict]:
    """Which personas to make and the project each walks."""
    projects = projects_of(export)
    first = project or (sorted(projects)[0] if projects else None)
    rows = [{"name": "admin", "groups": ["/platform-admins"], "project": first,
             "why": "installation administrator (/platform-admins)"}]
    for g in viewer_grants(export):
        mine = sorted(p for p, o in projects.items() if o == g["organization"])
        if mine:
            pick = project if project in mine else mine[0]
            rows.append({"name": "viewer", "groups": [g["group"]], "project": pick,
                         "why": f"viewer of Organization {g['organization']} (RoleBinding {g['binding']})"})
            break
    rows.append({"name": "outsider", "groups": [], "project": first, "why": "signed in, no grant"})
    return rows


if __name__ == "__main__":
    import sys
    print(json.dumps(plan(Path(sys.argv[1]) if len(sys.argv) > 1 else None, None), indent=1))
