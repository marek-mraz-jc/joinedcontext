#!/usr/bin/env python3
"""Rewrites tests/portal-contract from a run of tools/portal-dump (the Portal's own render).

  cargo build --manifest-path tools/portal-dump/Cargo.toml   # links crates/portal
  target/debug/portal-dump <deployment>/components/context-gateway/seed > dump.jsonl
  python3 scripts/portal-contract/regen.py dump.jsonl <deployment> <portal commit>

Writes streams.jsonl.gz (one stream per line, each `json_schema.schema` replaced by
`sha256:<16 hex>`), schemas.json.gz (those schemas by key), calls.txt (every other call the
Portal made to its runner while converging the seed), resources.yaml (the runner's shared
resources) and SOURCE.md (what produced them).
"""
import gzip
import hashlib
import json
import os
import shutil
import subprocess
import sys

dump, deployment, portal = sys.argv[1], sys.argv[2], sys.argv[3]
out = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
                   "tests", "portal-contract")
schemas, streams, calls, outcomes = {}, [], [], {}


def walk(node):
    if isinstance(node, dict):
        for k, v in node.items():
            if k == "json_schema" and isinstance(v, dict) and isinstance(v.get("schema"), str):
                key = hashlib.sha256(v["schema"].encode()).hexdigest()[:16]
                schemas[key] = v["schema"]
                v["schema"] = "sha256:" + key
            else:
                walk(v)
    elif isinstance(node, list):
        for v in node:
            walk(v)


for line in open(dump):
    d = json.loads(line)
    if "stream" in d:
        walk(d["stream"])
        streams.append(d)
    elif "call" in d:
        method, path = d["call"].split("?")[0].split(" ", 1)
        parts = path.split("/")
        if len(parts) > 2 and parts[1] in ("streams", "resources"):
            parts[-1] = "{project}.{name}" if parts[1] == "streams" else "{project}__pipeline_egress"
        calls.append(f"{method} {'/'.join(parts)}")
    else:
        outcomes[(d["project"], d["name"])] = d["outcome"]
for s in streams:
    s["outcome"] = outcomes.get((s["project"], s["name"]), "")
streams.sort(key=lambda s: s["id"])
os.makedirs(out, exist_ok=True)
with gzip.open(os.path.join(out, "streams.jsonl.gz"), "wt", compresslevel=9) as f:
    for s in streams:
        f.write(json.dumps(s, sort_keys=True, ensure_ascii=False) + "\n")
with gzip.open(os.path.join(out, "schemas.json.gz"), "wt", compresslevel=9) as f:
    json.dump(schemas, f, sort_keys=True)
with open(os.path.join(out, "calls.txt"), "w") as f:
    for c in sorted(set(calls)):
        f.write(f"{calls.count(c):5} {c}\n")
shutil.copy(os.path.join(deployment, "components/pipeline-runner/streams/resources.yaml"),
            os.path.join(out, "resources.yaml"))
# The runner mounts the GTFS-RT schema alone at /proto (development-values `import_paths: [/proto]`).
os.makedirs(os.path.join(out, "proto"), exist_ok=True)
shutil.copy(os.path.join(deployment, "components/pipeline-runner/streams/gtfs-realtime.proto"),
            os.path.join(out, "proto", "gtfs-realtime.proto"))
dep = subprocess.run(["git", "-C", deployment, "rev-parse", "--short", "HEAD"],
                     capture_output=True, text=True).stdout.strip()
with open(os.path.join(out, "SOURCE.md"), "w") as f:
    f.write(f"# Portal contract fixtures\n\n{len(streams)} streams the Portal (crates/portal at {portal}) "
            f"hands its runner for the seed of joinedcontext-deployment {dep}, rendered by "
            "tools/portal-dump (the Portal's StreamDeployer::converge, pipeline identity and validation "
            "on) and written by scripts/portal-contract/regen.py. resources.yaml is that deployment's "
            "components/pipeline-runner/streams/resources.yaml; proto/ holds its "
            "components/pipeline-runner/streams/gtfs-realtime.proto (Apache-2.0, google/transit), which "
            "the runner mounts at /proto.\n")
print(len(streams), "streams,", len(schemas), "schemas,", len(set(calls)), "distinct calls")
