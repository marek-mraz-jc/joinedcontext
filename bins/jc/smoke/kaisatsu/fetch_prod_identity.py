#!/usr/bin/env python3
"""Prod's sign-in documents, fetched once by hand for the `kaisatsu` suite (which never contacts prod).

   python3 bins/jc/smoke/kaisatsu/fetch_prod_identity.py --out bins/jc/smoke/kaisatsu/fixtures/prod-identity-<date>.json

Anonymous GETs only, at most one a second, never a sign-in, a token or a registration:
- the realm's OpenID configuration (idm.<domain>/realms/<realm>/.well-known/openid-configuration),
  kept whole except the per-Endpoint `endpoint:*` scopes (counted);
- the gateway's RFC 8414 metadata (api.<domain>/.well-known/oauth-authorization-server);
- the authorize pages of PROBES without signing in: status, and what the page is (a login form,
  Keycloak's error page and its message, or the gateway's refusal text). No cookie, code or
  Location query is kept."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sys
import time
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from common import Http

# An S256 challenge of a verifier nobody holds: the probes never take a code to the token endpoint.
CHALLENGE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
CIMD_CLAUDE = "https://claude.ai/oauth/mcp-oauth-client-metadata"
CLAUDE_CALLBACK = "https://claude.ai/api/mcp/auth_callback"
CONNECTOR_ID = "a" * 26  # well-formed (connectors::is_id), names no connector

# id -> (door, client_id, redirect_uri or None for the Portal's own address, scope). `door` kc is
# the realm's authorize, gw the gateway's. The rehearsal asks the same, on its own addresses.
PROBES = {
    "portal-ui": ("kc", "portal-ui", None, "openid"),
    "mcp-local": ("kc", "mcp-local", "http://localhost/callback", "openid mcp:portal"),
    "mcp-hub": ("kc", "mcp-hub", CLAUDE_CALLBACK, "openid offline_access"),
    "cimd-claude": ("kc", CIMD_CLAUDE, CLAUDE_CALLBACK, f"connector:{CONNECTOR_ID} offline_access"),
    "gateway-unknown-client": ("gw", "not-issued-by-this-server", "http://localhost/callback", ""),
}


def authorize_query(client_id: str, redirect: str, scope: str) -> str:
    q = {"client_id": client_id, "redirect_uri": redirect, "response_type": "code", "state": "smoke",
         "code_challenge": CHALLENGE, "code_challenge_method": "S256"}
    if scope:
        q["scope"] = scope
    return urllib.parse.urlencode(q)


def page_kind(status: int, headers: dict, raw: bytes) -> dict:
    """What an authorize answer is, without anything it carried for the caller."""
    text = raw.decode(errors="replace")
    loc = headers.get("location") or ""
    out = {"status": status}
    if loc:
        u = urllib.parse.urlparse(loc)
        out["redirect"] = f"{u.scheme}://{u.netloc}{u.path}"
        out["redirectParams"] = sorted(urllib.parse.parse_qs(u.query))
    if re.search(r'name="username"', text) and re.search(r'name="password"', text):
        out["page"] = "login"
    elif "The request was refused:" in text:
        out["page"] = "refusal"
        out["message"] = text.strip()[:200]
    elif status >= 400:
        out["page"] = "error"
        m = re.search(r'kc-error-message[^>]*>\s*(?:<[^>]+>\s*)*([^<]+)', text)
        out["message"] = (m.group(1).strip() if m else text.strip()[:120])[:200]
    else:
        out["page"] = "other"
    return out


class Paced:
    def __init__(self, gap: float):
        self.gap, self.last = gap, 0.0

    def get(self, url: str):
        wait = self.last + self.gap - time.monotonic()
        if wait > 0:
            time.sleep(wait)
        self.last = time.monotonic()
        return Http("", timeout=60).call("GET", url)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--domain", default="smartcity.joinedcontext.com")
    ap.add_argument("--realm", default="smartcity")
    ap.add_argument("--out", required=True)
    ap.add_argument("--gap", type=float, default=1.0)
    a = ap.parse_args(argv)
    if a.gap < 1:
        ap.error("--gap below 1 s: prod is read at most once a second")
    p = Paced(a.gap)
    idm, api, portal = f"https://idm.{a.domain}", f"https://api.{a.domain}", f"https://portal.{a.domain}"
    st, _, raw = p.get(f"{idm}/realms/{a.realm}/.well-known/openid-configuration")
    if st != 200:
        raise SystemExit(f"openid-configuration: {st}")
    oidc = json.loads(raw)
    scopes = oidc.get("scopes_supported") or []
    oidc["scopes_supported"] = [s for s in scopes if not s.startswith("endpoint:")]
    oidc["x-endpointScopes"] = len(scopes) - len(oidc["scopes_supported"])
    st, _, raw = p.get(f"{api}/.well-known/oauth-authorization-server")
    if st != 200:
        raise SystemExit(f"oauth-authorization-server: {st}")
    gateway = json.loads(raw)
    probes = {}
    for pid, (door, client, redirect, scope) in PROBES.items():
        base = f"{idm}/realms/{a.realm}/protocol/openid-connect/auth" if door == "kc" else f"{api}/oauth/authorize"
        st, h, raw = p.get(f"{base}?{authorize_query(client, redirect or portal + '/', scope)}")
        probes[pid] = page_kind(st, h, raw)
        print(pid, probes[pid], flush=True)
    doc = {"fetchedAt": dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"), "domain": a.domain,
           "realm": a.realm, "issuer": oidc.get("issuer"), "gatewayIssuer": gateway.get("issuer"),
           "openidConfiguration": oidc, "gatewayMetadata": gateway, "authorize": probes}
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_text(json.dumps(doc, indent=1, ensure_ascii=False) + "\n")
    print(f"wrote {a.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
