#!/usr/bin/env python3
"""Every policy refusal of an ETSI run is a 403 problem naming no entity (T-4855).

Usage: refusals.py <results dir>

Reads the Robot console logs (`*-console.log`) dev/etsi-pipeline.sh writes, where the
suite prints each response as `Response ->` and a JSON object. Every response with
status 403 must be the AccessDenied problem with one of the engine's two reasons and
must carry no entity id, type or attribute of the request (no `urn:` value at all).
Red when one does, or when the run holds no 403 (the restricted cell refused nothing).
"""
import glob
import json
import sys

REASONS = {"no policy grants this operation", "a policy prohibits this operation"}
TYPE = "urn:antares:error:AccessDenied"


def responses(text):
    """Each JSON object printed after a `Response ->` line."""
    dec = json.JSONDecoder()
    at = 0
    while (at := text.find("Response ->", at)) >= 0:
        start = text.find("{", at)
        at += 1
        if start < 0:
            return
        try:
            yield dec.raw_decode(text, start)[0]
        except ValueError:
            continue


def problems(resp):
    """Why one 403 response is not a clean refusal; empty when it is."""
    body = resp.get("body")
    if not isinstance(body, dict):
        return [f"body is not a problem object: {str(body)[:200]}"]
    out = []
    if body.get("type") != TYPE or body.get("status") != 403:
        out.append(f"not the AccessDenied problem: {body}")
    if body.get("detail") not in REASONS:
        out.append(f"detail is not one of the engine's reasons: {body.get('detail')!r}")
    if set(body) - {"type", "title", "status", "detail"}:
        out.append(f"members beyond the problem: {sorted(set(body) - {'type', 'title', 'status', 'detail'})}")
    if "urn:" in json.dumps({k: v for k, v in body.items() if k != "type"}):
        out.append("names a URN")
    return out


def main(results):
    seen, red = 0, []
    for log in sorted(glob.glob(f"{results}/*-console.log")):
        with open(log, errors="replace") as fh:
            text = fh.read()
        for resp in responses(text):
            if resp.get("status_code") != 403:
                continue
            seen += 1
            red += [f"{log}: {resp.get('url')}: {p}" for p in problems(resp)]
    print(f"{seen} refusals read")
    if not seen:
        red.append("no 403 in the run: the policy refused nothing")
    for line in red:
        print(line)
    return 1 if red else 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    sys.exit(main(sys.argv[1]))
