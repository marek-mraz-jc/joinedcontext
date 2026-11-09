"""The test person the closed-Endpoint checks read with, for a rehearsal's realm `jc`.

    python3 -I person.py realm --export <export> --out-dir <kaisatsu import dir> --secrets-dir <private dir>
    export SMOKE_PERSON_PASSWORD="$(cat <private dir>/smoke-person-password)" \
           SMOKE_PERSON_CLIENT_SECRET="$(cat <private dir>/smoke-person-client-secret)"
    (then restart jc in that environment, so kaisatsu applies the file)
    python3 -I person.py token --base-url http://localhost:42101 --secrets-dir <private dir> --out <dir>/person-token

`realm` writes `90-smoke-endpoints.json` (realm `jc`): every group a Policy bound to a closed Endpoint
grants and `{project}-readers` of each closed Endpoint's project (the person joins each), the person, and a confidential client with one audience mapper per
closed Endpoint slug plus a groups mapper. A kaisatsu import file, because the Portal's admin client
holds no manage-clients. Password and client secret are fresh random values in two 0600 files of
--secrets-dir; the import file names them `$(env:SMOKE_PERSON_*)` (kaisatsu refuses literal
secrets, and `$(file:)` needs a secrets directory jc does not configure).
`token` makes a password-grant access token (0600)."""

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
USER, CLIENT = "smoke-endpoints-person", "smoke-endpoints"
PASSWORD_FILE, SECRET_FILE = "smoke-person-password", "smoke-person-client-secret"
PASSWORD_ENV, SECRET_ENV = "SMOKE_PERSON_PASSWORD", "SMOKE_PERSON_CLIENT_SECRET"


def write_private(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(text)


def realm_file(eps) -> dict:
    closed = [e for e in eps if e.closed]
    slugs = sorted({e.slug for e in closed})
    # The Policies' groups, and `{project}-readers` of each closed Endpoint's project: the gateway
    # admits a person on a closed Endpoint only from a project its groups name (PF-87).
    groups = sorted({str(g.assignee.get("id")).lstrip("/") for e in closed for g in e.grants
                     if g.reads() and g.assignee.get("kind") == "group"}
                    | {f"{e.project}-readers" for e in closed})
    mappers = [{"name": f"aud-{s}", "protocol": "openid-connect", "protocolMapper": "oidc-audience-mapper",
                "consentRequired": False,
                "config": {"included.custom.audience": s, "access.token.claim": "true", "id.token.claim": "false"}}
               for s in slugs]
    mappers.append({"name": "groups", "protocol": "openid-connect", "protocolMapper": "oidc-group-membership-mapper",
                    "consentRequired": False,
                    "config": {"claim.name": "groups", "full.path": "true", "access.token.claim": "true",
                               "id.token.claim": "false"}})
    return {
        "realm": REALM, "enabled": True,
        "groups": [{"name": g} for g in groups],
        "clients": [{"clientId": CLIENT, "enabled": True, "protocol": "openid-connect", "publicClient": False,
                     "clientAuthenticatorType": "client-secret", "secret": f"$(env:{SECRET_ENV})", "standardFlowEnabled": False,
                     "directAccessGrantsEnabled": True, "serviceAccountsEnabled": False, "fullScopeAllowed": False,
                     "redirectUris": [], "webOrigins": [], "defaultClientScopes": ["basic"],
                     "protocolMappers": mappers}],
        "users": [{"username": USER, "enabled": True, "email": f"{USER}@smoke.invalid", "emailVerified": True,
                   "firstName": "Smoke", "lastName": "Endpoints", "groups": [f"/{g}" for g in groups],
                   "credentials": [{"type": "password", "value": f"$(env:{PASSWORD_ENV})",
                                    "temporary": False}]}],
    }


def cmd_realm(a) -> int:
    eps = ex.load_endpoints(Path(a.export) / "gitea" / "files")
    pw, sec = Path(a.secrets_dir) / PASSWORD_FILE, Path(a.secrets_dir) / SECRET_FILE
    for f, value in ((pw, secrets.token_urlsafe(18) + "-7aZ"), (sec, secrets.token_urlsafe(32))):
        if not f.exists():  # kept across reruns: kaisatsu holds the person it made first
            write_private(f, value)
    realm = realm_file(eps)
    write_private(Path(a.out_dir) / "90-smoke-endpoints.json", json.dumps(realm, indent=1))
    aud = len(realm["clients"][0]["protocolMappers"]) - 1
    print(json.dumps({"realm file": str(Path(a.out_dir) / "90-smoke-endpoints.json"), "audience": aud,
                      "groups": [g["name"] for g in realm["groups"]],
                      "before starting jc": f'export {PASSWORD_ENV}="$(cat {pw})" {SECRET_ENV}="$(cat {sec})"'}))
    return 0


def cmd_token(a) -> int:
    loopback_only(a.base_url)
    d = Path(a.secrets_dir)
    s = {"username": USER, "client_id": CLIENT, "password": (d / PASSWORD_FILE).read_text().strip(),
         "client_secret": (d / SECRET_FILE).read_text().strip()}
    st, _, raw = Http(a.base_url).call(
        "POST", f"/id/realms/{REALM}/protocol/openid-connect/token",
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        body=urllib.parse.urlencode({"grant_type": "password", "client_id": s["client_id"],
                                     "client_secret": s["client_secret"], "username": s["username"],
                                     "password": s["password"]}).encode())
    tok = as_json(raw) or {}
    if st != 200 or "access_token" not in tok:
        print(f"person token: {st} {str(tok)[:200]}", file=sys.stderr)
        return 1
    write_private(Path(a.out), tok["access_token"])
    print(json.dumps({"token": a.out, "expires_in": tok.get("expires_in")}))
    return 0


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("realm")
    r.add_argument("--export", required=True)
    r.add_argument("--out-dir", required=True)
    r.add_argument("--secrets-dir", required=True)
    t = sub.add_parser("token")
    t.add_argument("--base-url", required=True)
    t.add_argument("--secrets-dir", required=True)
    t.add_argument("--out", required=True)
    a = p.parse_args(argv)
    return cmd_realm(a) if a.cmd == "realm" else cmd_token(a)


if __name__ == "__main__":
    sys.exit(main())
