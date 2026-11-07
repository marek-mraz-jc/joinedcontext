#!/usr/bin/env python3
"""The builder's seed stage builds each Rust App offline from the App's own Cargo.lock, with only
the crates the `crates` stage fetched: the root lock and the wasm/server locks. A crate version an
App lock names that none of those locks names fails the image build (rustls 0.23.45 after T-4150
bumped the root lock alone, image run 38797909402). Bump an App's lock with the root lock.
"""
import glob, pathlib, re, sys, tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]
FETCHED = ["Cargo.lock", "sdk/examples/react-rust-wasm/wasm/Cargo.lock", "apps/*/wasm/Cargo.lock",
           "sdk/examples/*/server/Cargo.lock", "sdk/template-wasm/server/Cargo.lock",
           "apps/*/server/Cargo.lock"]


def crates(lock):
    return {(p["name"], p["version"]) for p in tomllib.loads(lock.read_text()).get("package", [])
            if p.get("source", "").startswith("registry+")}


fetched = set()
for pattern in FETCHED:
    for lock in glob.glob(str(ROOT / pattern)):
        fetched |= crates(pathlib.Path(lock))
seeded = re.findall(r"^COPY (apps/[^/\s]+)/Cargo\.toml \1/Cargo\.lock /manifests/",
                    (ROOT / "builder/Dockerfile").read_text(), re.M)
if not seeded:
    sys.exit("builder/Dockerfile copies no App lock into the seed stage: update this check")
bad = 0
for app in seeded:
    for name, version in sorted(crates(ROOT / app / "Cargo.lock") - fetched):
        print(f"{app}/Cargo.lock: {name} {version} is in no fetched lock (root has another version)")
        bad += 1
sys.exit(1 if bad else 0)
