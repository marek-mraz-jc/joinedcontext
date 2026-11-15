#!/usr/bin/env python3
"""Prepare the rendered manifests for the credential scan.

`helm template` runs the in-cluster secret generator without a cluster, so every
render invents fresh random passwords. They are throwaway (on apply the generator
reuses the live Secret), and scanning them only produces noise, so their values are
redacted here. Everything else — including Secrets shipped by third-party charts —
reaches gitleaks unchanged, and a hand-written `stringData:` block fails outright:
that is a plaintext credential inside a manifest.

usage: check-rendered-secrets.py <rendered.yaml> <sanitized.yaml>
"""
import sys

import yaml

# The charts that generate a Secret's values at install and keep the live ones (`lookup`): the
# platform's secrets component and the BBSK demo's own base chart (demos/bbsk/chart, T-4687).
GENERATOR_CHART_PREFIXES = ("secrets-", "demo-bbsk-base-")

# Third-party charts that ship non-credential content as `stringData`: Gitea keeps its
# app.ini sections and its init shell scripts there. Their credentials are not in them —
# the database password and the OIDC client secret arrive as secretKeyRef environment
# variables, and the scripts only reference the variable names. The content still reaches
# gitleaks below, unredacted and in plaintext, which is the only form the scanner can read
# at all: this list narrows the blunt rule, it does not skip the scan.
STRINGDATA_ALLOWED = {
    ("gitea-", "gitea"),
    ("gitea-", "gitea-init"),
    ("gitea-", "gitea-inline-config"),
    # The APISIX rule file helm seeds until the Portal composes it (ADR-N-030, AP-112): the
    # shared routes, whose secrets are `${{ENV}}` references; the per-App client secrets are
    # only ever written into it at runtime by the Portal.
    ("configuration-", "apisix-standalone-config"),
}


def main(src: str, dst: str) -> int:
    docs = list(yaml.safe_load_all(open(src)))
    problems = []
    for doc in docs:
        if not isinstance(doc, dict) or doc.get("kind") != "Secret":
            continue
        meta = doc.get("metadata") or {}
        name = f"{meta.get('namespace', '-')}/{meta.get('name')}"
        chart = (meta.get("labels") or {}).get("helm.sh/chart", "")
        allowed = any(
            chart.startswith(prefix) and meta.get("name") == secret
            for prefix, secret in STRINGDATA_ALLOWED
        )
        if doc.get("stringData") and not allowed:
            problems.append(f"{name}: stringData is a plaintext credential in a manifest")
        if chart.startswith(GENERATOR_CHART_PREFIXES):
            doc["data"] = {key: "REDACTED" for key in doc.get("data") or {}}

    if problems:
        print("\n".join(f"::error::{p}" for p in problems), file=sys.stderr)
        return 1

    with open(dst, "w") as out:
        yaml.safe_dump_all(docs, out)
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1], sys.argv[2]))
