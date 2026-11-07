#!/usr/bin/env bash
# Regenerates the gen-linkml fixtures of crates/linkml-model/tests (T-5207) with the
# LinkML generator on PATH (linkml 1.12 in the CI image): its JSON output, and its YAML
# output read as JSON, as a YAML-reading client hands it on.
set -euo pipefail
dir="$(cd "$(dirname "$0")/.." && pwd)/crates/linkml-model/tests/fixtures"
cd "$dir"
gen-linkml --format json gen-linkml-source.yaml 2>/dev/null >gen-linkml.json
gen-linkml --format yaml gen-linkml-source.yaml 2>/dev/null |
  python3 -c 'import json, sys, yaml; json.dump(yaml.safe_load(sys.stdin), sys.stdout, indent=2, default=str)' >gen-linkml-yaml.json
