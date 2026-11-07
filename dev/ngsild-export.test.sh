#!/usr/bin/env bash
# dev/ngsild-export.py (T-3913): the exported workspace holds the five crates and nothing that
# reads the vendored suite, and builds and passes its tests on its own with the pruned lockfile.
set -euo pipefail
out="$(mktemp -d)/ngsild"
trap 'rm -rf "${out:?}"' EXIT
python3 "$(dirname "$0")/ngsild-export.py" "$out"
cd "$out"
test -f crates/antares-jsonld/src/loader.rs
if grep -rl --include='*.rs' ngsi-ld-test-suite crates/*/tests crates/*/examples; then exit 1; fi
grep -q '^repository = "https://github.com/marek-mraz-jc/ngsild"$' Cargo.toml
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"
cargo fmt --all --check
cargo test --workspace --all-features --locked
echo "ngsild-export: ok"
