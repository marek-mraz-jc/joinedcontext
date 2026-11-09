#!/usr/bin/env python3
"""Loads a public CKAN dump (package_search rows=1000 + organization_list all_fields) into a
rehearsal's dataroom, so the catalogue pages compare on the same datasets.

  ckan_fixture.py --dump <dir with packages.json, orgs.json> --base http://127.0.0.1:<port> --token-file <state>/catalogue-token

A UI-parity fixture only: on a real swap the Portal publishes the datasets from the Endpoints. Writes
go only to a loopback base. Prints one line per refusal and a summary.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlparse

DROP = {"id", "owner_org", "organization", "creator_user_id", "metadata_created", "metadata_modified", "num_resources",
        "num_tags", "relationships_as_object", "relationships_as_subject", "isopen", "license_title", "license_url", "state",
        "revision_id"}
DROP_RES = {"id", "package_id", "created", "metadata_modified", "cache_last_updated", "cache_url", "datastore_active",
            "position", "state", "revision_id", "url_type"}


def loopback_only(base: str):
    if urlparse(base).hostname not in ("127.0.0.1", "localhost", "::1"):
        raise SystemExit(f"refused: {base} is not loopback")


def call(base, token, action, body):
    req = urllib.request.Request(f"{base}/opendata/api/3/action/{action}", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json", "Authorization": token}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        return e.code, (e.read() or b"")[:300].decode(errors="replace")


def package_body(p: dict) -> dict:
    b = {k: v for k, v in p.items() if k not in DROP and v not in (None, "")}
    b["owner_org"] = p["organization"]["name"] if p.get("organization") else None
    b["tags"] = [{"name": t["name"]} for t in p.get("tags", [])]
    b["groups"] = [{"name": g["name"]} for g in p.get("groups", [])]
    b["resources"] = [{k: v for k, v in r.items() if k not in DROP_RES and v not in (None, "")} for r in p.get("resources", [])]
    return b


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--dump", required=True)
    ap.add_argument("--base", required=True)
    ap.add_argument("--token-file", required=True)
    a = ap.parse_args(argv)
    loopback_only(a.base)
    token = Path(a.token_file).read_text().strip()
    dump = Path(a.dump)
    counts = {"org": 0, "package": 0, "refused": 0}
    for o in json.loads((dump / "orgs.json").read_text())["result"]:
        body = {k: o[k] for k in ("name", "title", "description", "image_url") if o.get(k)}
        st, ans = call(a.base, token, "organization_create", body)
        if st >= 300 and "already" not in str(ans).lower() and "exist" not in str(ans).lower():
            counts["refused"] += 1
            print(f"organization_create {o['name']}: {st} {str(ans)[:200]}", file=sys.stderr)
        else:
            counts["org"] += 1
    for p in json.loads((dump / "packages.json").read_text())["result"]["results"]:
        body = package_body(p)
        st, ans = call(a.base, token, "package_create", body)
        if st >= 300:
            st2, ans2 = call(a.base, token, "package_update", {**body, "id": body["name"]})
            if st2 >= 300:
                counts["refused"] += 1
                print(f"package_create {p['name']}: {st} {str(ans)[:200]} / update {st2} {str(ans2)[:120]}", file=sys.stderr)
                continue
        counts["package"] += 1
    print(json.dumps(counts))


if __name__ == "__main__":
    main()
