"""Make the test person the closed-Endpoint checks read with, in a rehearsal's realm `jc`:

    python3 -I person.py --base-url http://localhost:42101 --admin-secret-file <state>/portal-admin-client-secret \
        --export <export> --out <dir>/person-token

From the export it takes every closed Endpoint (their slugs become the token's audience) and every
group a Policy bound to one of them grants (the person joins each, made when missing). It creates the
person and a confidential client with one audience mapper per slug plus a groups mapper, then writes
a password-grant access token to --out (0600). Password and client secret live in memory only.
Loopback bases only: it writes to the realm."""

from __future__ import annotations

import argparse
import json
import os
import secrets
import sys
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))

import export as ex  # noqa: E402
from common import Http, as_json, loopback_only  # noqa: E402

REALM = "jc"


def form(h: Http, path: str, fields: dict):
    st, _, raw = h.call("POST", path, headers={"Content-Type": "application/x-www-form-urlencoded"},
                        body=urllib.parse.urlencode(fields).encode())
    return st, as_json(raw)


def must(st, ok, what, body):
    if st not in ok:
        raise SystemExit(f"{what}: {st} {str(body)[:300]}")


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--base-url", required=True)
    p.add_argument("--admin-client", default="portal-reconciler")
    p.add_argument("--admin-secret-file", required=True)
    p.add_argument("--export", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--username", default="smoke-endpoints-person")
    p.add_argument("--client-id", default="smoke-endpoints")
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    h = Http(a.base_url)
    eps = [e for e in ex.load_endpoints(Path(a.export) / "gitea" / "files") if e.closed]
    slugs = sorted({e.slug for e in eps})
    groups = sorted({str(g.assignee.get("id")).lstrip("/") for e in eps for g in e.grants
                     if g.reads() and g.assignee.get("kind") == "group"})
    st, tok = form(h, f"/id/realms/{REALM}/protocol/openid-connect/token", {
        "grant_type": "client_credentials", "client_id": a.admin_client,
        "client_secret": Path(a.admin_secret_file).read_text().strip()})
    must(st, (200,), f"{a.admin_client} token", tok)
    admin = tok["access_token"]
    api = f"/id/admin/realms/{REALM}"

    def j(method, path, body=None):
        st, _, raw = h.call(method, api + path, token=admin, body=body)
        return st, as_json(raw)

    for g in groups:
        st, body = j("POST", "/groups", {"name": g})
        must(st, (201, 204, 409), f"group {g}", body)
    st, tree = j("GET", "/groups?briefRepresentation=false&max=10000")
    ids, todo = {}, list(tree or [])
    while todo:
        g = todo.pop()
        ids.setdefault(g["name"], g["id"])
        todo += g.get("subGroups") or []
    password = secrets.token_urlsafe(18) + "-7aZ"
    st, body = j("POST", "/users", {"username": a.username, "enabled": True, "emailVerified": True,
                                    "email": f"{a.username}@smoke.invalid", "firstName": "Smoke", "lastName": "Endpoints"})
    must(st, (201, 409), "person", body)
    st, users = j("GET", f"/users?username={a.username}&exact=true")
    uid = users[0]["id"]
    st, body = j("PUT", f"/users/{uid}/reset-password", {"type": "password", "value": password, "temporary": False})
    must(st, (200, 204), "password", body)
    for g in groups:
        st, body = j("PUT", f"/users/{uid}/groups/{ids[g]}")
        must(st, (200, 204), f"join {g}", body)
    secret = secrets.token_urlsafe(32)
    mappers = [{"name": f"aud-{s}", "protocol": "openid-connect", "protocolMapper": "oidc-audience-mapper",
                "config": {"included.custom.audience": s, "access.token.claim": "true", "id.token.claim": "false"}}
               for s in slugs]
    mappers.append({"name": "groups", "protocol": "openid-connect", "protocolMapper": "oidc-group-membership-mapper",
                    "config": {"claim.name": "groups", "full.path": "true", "access.token.claim": "true",
                               "id.token.claim": "false"}})
    client = {"clientId": a.client_id, "enabled": True, "protocol": "openid-connect", "publicClient": False,
              "clientAuthenticatorType": "client-secret", "secret": secret, "standardFlowEnabled": False,
              "directAccessGrantsEnabled": True, "serviceAccountsEnabled": False, "fullScopeAllowed": False,
              "redirectUris": [], "webOrigins": [], "defaultClientScopes": ["basic"], "protocolMappers": mappers}
    st, found = j("GET", f"/clients?clientId={a.client_id}")
    if found:
        st, body = j("PUT", f"/clients/{found[0]['id']}", {**found[0], **client})
    else:
        st, body = j("POST", "/clients", client)
    must(st, (200, 201, 204), "client", body)
    st, tok = form(h, f"/id/realms/{REALM}/protocol/openid-connect/token", {
        "grant_type": "password", "client_id": a.client_id, "client_secret": secret,
        "username": a.username, "password": password})
    must(st, (200,), "person token", tok)
    out = Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(tok["access_token"])
    print(json.dumps({"token": str(out), "audience": len(slugs), "groups": groups}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
