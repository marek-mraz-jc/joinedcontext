#!/usr/bin/env python3
"""Module rules M1, M2 and M3 (ADR-0027 section 2, surfaces-threat-model 9.3, 9.4,
10.1 and the 11.1 row `default-features = false`), checked on the output of
`cargo metadata --format-version 1 --locked --no-deps`.

Every workspace package declares `[package.metadata.antares] role` (core, ngsild,
ops, surface, protocol, addon). From the roles:

  M1  a protocol crate depends on no `antares-*` crate, in any dependency kind;
      a surface crate is an optional dependency of `antares-broker` behind its
      own feature (one feature per surface, containing `dep:<crate>`); the
      broker's `default` feature reaches no surface, ops or protocol crate.
  M2  a surface depends inside the workspace only on `antares-ops` and the
      protocol crates it lists (dev: `antares-ops` alone); `antares-ops`
      never depends on the store, SQL, bus, notifier, a surface or a protocol
      crate, and never re-exports the store, SQL or `AppState` from its source
      (threat F1: the store is unreachable from a surface by dependency).
  M3  an ngsild crate (pure NGSI-LD, reusable outside the broker, T-3910)
      depends inside the workspace only on ngsild crates, and outside it only
      on the crates NGSILD_EXTERNAL names; any other crate is an optional
      dependency its `default` feature does not turn on (T-3911's `fetch`).
  11.1 every registry dependency of an ops, surface or protocol crate has
      `default-features = false` unless `default-features-ok` names it with a
      reason.
  10.1 every ops, surface or protocol crate carries the full `[lints]` table.

`--self-test` runs every check against a synthetic workspace with exactly one
violation per red condition and fails unless each is reported, so the gate
cannot pass by never matching (the dev/check-no-addon.sh discipline).
"""

import copy
import fnmatch
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import tomllib

ROLES = {"core", "ngsild", "ops", "surface", "protocol", "addon"}
BROKER = "antares-broker"
OPS = "antares-ops"
OPS_FORBIDDEN = {"antares-store", "antares-sql", "antares-bus", "antares-notifier"}

# M3: what an ngsild crate may link by default. Patterns are fnmatch globs.
NGSILD_EXTERNAL = ("serde", "serde_json", "thiserror", "chrono", "regex", "geo*", "unicode-*", "uuid")

# 10.1: the table each new crate carries instead of `workspace = true`.
LINTS = {
    "rust": {
        "unsafe_code": "forbid",
        "missing_docs": "deny",
        "unexpected_cfgs": {"level": "warn", "check-cfg": ["cfg(tokio_unstable)"]},
    },
    "clippy": {
        "pedantic": {"level": "deny", "priority": -1},
        **{
            lint: "deny"
            for lint in (
                "unwrap_used", "expect_used", "panic", "indexing_slicing",
                "arithmetic_side_effects", "todo", "unimplemented", "dbg_macro",
                "print_stdout", "print_stderr",
            )
        },
    },
    "rustdoc": {"broken_intra_doc_links": "deny"},
}

# 9.4: the re-exports that would hand a surface the store back (F1).
OPS_LEAKS = [
    re.compile(r"\bpub\s+use\s+antares_(store|sql)\b"),
    re.compile(r"\bpub\s+use\s+antares_api::(\{[^}]*\b)?(AppState|state)\b"),
    re.compile(r"\bpub\s+\w+\s*:\s*[^;,{}]*\bAppState\b"),
    re.compile(r"\bpub\s+(async\s+)?fn\s[^{;]*->[^{;]*\bAppState\b"),
]


def antares(pkg):
    return (pkg.get("metadata") or {}).get("antares") or {}


def feature_closure(features, start, optional_deps):
    """Features and `dep:` names `start` turns on (cargo's own resolution:
    `x/f` enables an optional dep `x`, `x?/f` does not, a bare name that is
    no feature but an optional dep is its implicit feature)."""
    on, deps, todo = set(), set(), list(start)
    while todo:
        f = todo.pop()
        if f.startswith("dep:"):
            deps.add(f[4:])
            continue
        if "/" in f:
            dep = f.split("/", 1)[0]
            if not dep.endswith("?") and dep in optional_deps:
                deps.add(dep)
            continue
        if f in on:
            continue
        on.add(f)
        if f in features:
            todo.extend(features[f])
        elif f in optional_deps:
            deps.add(f)
    return on, deps


def check(meta):
    """Every violation as (code, message)."""
    out = []
    pkgs = {p["name"]: p for p in meta["packages"]}
    role = {n: antares(p).get("role") for n, p in pkgs.items()}

    for name, r in sorted(role.items()):
        if r not in ROLES:
            out.append(("role", f"{name}: [package.metadata.antares] role is {r!r}, "
                                f"expected one of {sorted(ROLES)}"))

    def of(r):
        return sorted(n for n, x in role.items() if x == r)

    for name in of("protocol"):
        for d in pkgs[name]["dependencies"]:
            if d["name"].startswith(("antares-", "ngsild-")):
                out.append(("protocol-dep", f"{name} (protocol) depends on {d['name']} "
                                            f"({d['kind'] or 'normal'})"))

    for name in of("ngsild"):
        pkg = pkgs[name]
        optional = {d["name"] for d in pkg["dependencies"] if d["optional"]}
        _, by_default = feature_closure(pkg["features"], pkg["features"].get("default", []), optional)
        for d in pkg["dependencies"]:
            if d["kind"] == "dev":
                continue
            kind = d["kind"] or "normal"
            if d["name"] in pkgs:
                if role.get(d["name"]) != "ngsild":
                    out.append(("ngsild-dep", f"{name} (ngsild) depends on {d['name']} "
                                              f"({role.get(d['name'])}, {kind})"))
            elif not any(fnmatch.fnmatchcase(d["name"], g) for g in NGSILD_EXTERNAL):
                if not d["optional"]:
                    out.append(("ngsild-dep", f"{name} (ngsild) depends on {d['name']} ({kind}); "
                                              f"only {', '.join(NGSILD_EXTERNAL)} by default"))
                elif d["name"] in by_default:
                    out.append(("ngsild-dep", f"{name} (ngsild): its default features turn on "
                                              f"{d['name']}"))

    broker = pkgs.get(BROKER)
    if broker is None:
        out.append(("broker", f"{BROKER} is not a workspace package"))
        broker = {"features": {}, "dependencies": []}
    features = broker["features"]
    bdeps = [d for d in broker["dependencies"] if d["kind"] is None]
    optional = {d["name"] for d in bdeps if d["optional"]}

    seen = {}
    for name in of("surface"):
        feat = antares(pkgs[name]).get("feature")
        if name not in optional:
            out.append(("surface-optional", f"{name} (surface) is not an optional "
                                            f"dependency of {BROKER}"))
        if feat not in features:
            out.append(("surface-feature", f"{name}: feature {feat!r} is not a feature "
                                           f"of {BROKER}"))
        elif f"dep:{name}" not in features[feat]:
            out.append(("surface-feature", f"{name}: {BROKER} feature {feat!r} does not "
                                           f"contain dep:{name}"))
        if feat is not None and feat in seen:
            out.append(("surface-shared", f"{name} and {seen[feat]} name the same "
                                          f"feature {feat!r}"))
        seen.setdefault(feat, name)

    fenced = set(of("surface")) | set(of("ops")) | set(of("protocol"))
    on, deps = feature_closure(features, features.get("default", []), optional)
    for d in bdeps:
        if not d["optional"]:
            deps.add(d["name"])
    for feat in sorted(on & set(seen)):
        out.append(("default-reach", f"{BROKER} default turns on surface feature {feat!r}"))
    for dep in sorted(deps & fenced):
        out.append(("default-reach", f"{BROKER} default build links {dep} ({role[dep]})"))

    for name in of("surface"):
        listed = antares(pkgs[name]).get("protocols", [])
        for p in listed:
            if role.get(p) != "protocol":
                out.append(("surface-protocol", f"{name} lists {p} as a protocol, "
                                                f"its role is {role.get(p)!r}"))
        allowed = {OPS} | set(listed)
        for d in pkgs[name]["dependencies"]:
            if d["name"] not in pkgs:
                continue
            if d["kind"] == "dev":
                if d["name"] != OPS:
                    out.append(("surface-dev", f"{name} (surface) dev-depends on {d['name']}"))
            elif d["name"] not in allowed:
                out.append(("surface-dep", f"{name} (surface) depends on {d['name']} "
                                           f"({d['kind'] or 'normal'})"))

    for name in of("ops"):
        for d in pkgs[name]["dependencies"]:
            if d["kind"] == "dev":
                continue
            if d["name"] in OPS_FORBIDDEN or d["name"] in set(of("surface")) | set(of("protocol")):
                out.append(("ops-dep", f"{name} (ops) depends on {d['name']} "
                                       f"({d['kind'] or 'normal'})"))
        src = pathlib.Path(pkgs[name]["manifest_path"]).parent / "src"
        for f in sorted(src.rglob("*.rs")):
            for n, line in enumerate(f.read_text(encoding="utf-8").splitlines(), 1):
                if any(rx.search(line) for rx in OPS_LEAKS):
                    out.append(("ops-leak", f"{f}:{n}: {line.strip()}"))

    for name in sorted(fenced):
        ok = antares(pkgs[name]).get("default-features-ok", {})
        for d in pkgs[name]["dependencies"]:
            if d["kind"] == "dev" or d["source"] is None or not d["uses_default_features"]:
                continue
            reason = ok.get(d["name"]) if isinstance(ok, dict) else None
            if not (isinstance(reason, str) and reason.strip()):
                out.append(("default-features", f"{name} uses {d['name']} with default "
                                                f"features and no default-features-ok reason"))
        with open(pkgs[name]["manifest_path"], "rb") as fh:
            lints = tomllib.load(fh).get("lints", {})
        for table, want in LINTS.items():
            for lint, level in want.items():
                if lints.get(table, {}).get(lint) != level:
                    out.append(("lints", f"{name}: [lints.{table}] {lint} is "
                                         f"{lints.get(table, {}).get(lint)!r}, 10.1 wants {level!r}"))
    return out


def dep(name, kind=None, optional=False, default=True, source=None):
    return {"name": name, "kind": kind, "optional": optional,
            "uses_default_features": default, "source": source}


def lints_toml():
    def val(v):
        if isinstance(v, dict):
            return "{ " + ", ".join(f'{k} = {val(x)}' for k, x in v.items()) + " }"
        return json.dumps(v)
    return "".join(f"[lints.{t}]\n" + "".join(f'{k} = {val(v)}\n' for k, v in want.items())
                   for t, want in LINTS.items())


def synthetic(root):
    """A clean workspace with one crate of every role."""
    def pkg(name, role, deps=(), lints=False, **extra):
        d = root / name
        (d / "src").mkdir(parents=True, exist_ok=True)
        (d / "src" / "lib.rs").write_text("//! synthetic\n", encoding="utf-8")
        (d / "Cargo.toml").write_text(f'[package]\nname = "{name}"\n'
                                      + (lints_toml() if lints else ""), encoding="utf-8")
        return {"name": name, "manifest_path": str(d / "Cargo.toml"),
                "metadata": {"antares": {"role": role, **extra}},
                "features": {}, "dependencies": list(deps)}
    reg = "registry+https://github.com/rust-lang/crates.io-index"
    broker = pkg(BROKER, "core", [dep("antares-api"), dep("antares-store"),
                                  dep(OPS, optional=True), dep("antares-views", optional=True)])
    broker["features"] = {"default": ["mqtt"], "mqtt": ["antares-api/mqtt"],
                          "views": ["dep:antares-views", "antares-ops?/views"],
                          "full": ["views"]}
    return {"packages": [
        pkg("antares-api", "core", [dep("antares-store")]),
        pkg("antares-store", "core"),
        broker,
        pkg(OPS, "ops", [dep("antares-api"), dep("serde", default=False, source=reg)], lints=True),
        pkg("antares-views", "surface",
            [dep(OPS), dep("odata-url"), dep(OPS, kind="dev"),
             dep("tokio", source=reg)], lints=True,
            feature="views", protocols=["odata-url"],
            **{"default-features-ok": {"tokio": "runtime, its defaults are minimal"}}),
        pkg("odata-url", "protocol", [dep("serde", kind="dev", source=reg)], lints=True),
        pkg("antares-plugin-example", "addon", [dep("antares-store")]),
        pkg("antares-model", "ngsild", [dep("serde", source=reg), dep("geo-types", source=reg)]),
        {**pkg("antares-ql", "ngsild", [dep("antares-model"), dep("regex", source=reg),
                                        dep("tokio", optional=True, source=reg)]),
         "features": {"default": ["std"], "std": [], "fetch": ["dep:tokio"]}},
    ]}


def self_test():
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        base = synthetic(root)
        clean = check(base)
        if clean:
            print("self-test failed: the clean synthetic workspace is reported:", *clean, sep="\n  ")
            return 1

        def p(meta, name):
            return next(x for x in meta["packages"] if x["name"] == name)

        def edit(f):
            def run():
                meta = copy.deepcopy(base)
                f(meta)
                return meta
            return run

        def ops_file(text):
            def run():
                f = root / OPS / "src" / "leak.rs"
                f.write_text(text, encoding="utf-8")
                try:
                    return check(copy.deepcopy(base))
                finally:
                    f.unlink()
            return run

        def strip_lint():
            m = root / "odata-url" / "Cargo.toml"
            before = m.read_text(encoding="utf-8")
            m.write_text(before.replace('panic = "deny"\n', ""), encoding="utf-8")
            try:
                return check(copy.deepcopy(base))
            finally:
                m.write_text(before, encoding="utf-8")

        cases = [
            ("role", edit(lambda m: p(m, "antares-store")["metadata"].clear())),
            ("role", edit(lambda m: p(m, "antares-store")["metadata"]["antares"].update(role="plugin"))),
            ("protocol-dep", edit(lambda m: p(m, "odata-url")["dependencies"].append(dep("antares-model", kind="dev")))),
            ("surface-optional", edit(lambda m: p(m, BROKER)["dependencies"].__setitem__(3, dep("antares-views")))),
            ("surface-feature", edit(lambda m: p(m, "antares-views")["metadata"]["antares"].update(feature="odata"))),
            ("surface-feature", edit(lambda m: p(m, BROKER)["features"].update(views=["antares-ops?/views"]))),
            ("surface-shared", edit(lambda m: m["packages"].append(
                {**copy.deepcopy(p(m, "antares-views")), "name": "antares-mcp"}))),
            ("default-reach", edit(lambda m: p(m, BROKER)["features"].update(default=["mqtt", "full"]))),
            ("default-reach", edit(lambda m: p(m, BROKER)["features"].update(default=["antares-ops/views"]))),
            ("default-reach", edit(lambda m: p(m, BROKER)["dependencies"].append(dep("odata-url")))),
            ("surface-protocol", edit(lambda m: p(m, "antares-views")["metadata"]["antares"].update(protocols=["antares-api"]))),
            ("surface-dep", edit(lambda m: p(m, "antares-views")["dependencies"].append(dep("antares-store")))),
            ("surface-dep", edit(lambda m: p(m, "antares-views")["dependencies"].append(dep("antares-api", kind="build")))),
            ("surface-dev", edit(lambda m: p(m, "antares-views")["dependencies"].append(dep("antares-store", kind="dev")))),
            ("ops-dep", edit(lambda m: p(m, OPS)["dependencies"].append(dep("antares-sql")))),
            ("ops-dep", edit(lambda m: p(m, OPS)["dependencies"].append(dep("odata-url", kind="build")))),
            ("ops-leak", ops_file("pub use antares_store::Store;\n")),
            ("ops-leak", ops_file("pub use antares_api::{AppState, Router};\n")),
            ("ops-leak", ops_file("pub struct Ops {\n    pub state: AppState,\n}\n")),
            ("ops-leak", ops_file("pub fn state(&self) -> &AppState {\n")),
            ("default-features", edit(lambda m: p(m, OPS)["dependencies"].append(dep("regex", source="registry+x")))),
            ("default-features", edit(lambda m: p(m, "antares-views")["metadata"]["antares"].update(
                {"default-features-ok": {"tokio": " "}}))),
            ("lints", strip_lint),
            ("ngsild-dep", edit(lambda m: p(m, "antares-model")["dependencies"].append(dep("tokio", source="registry+x")))),
            ("ngsild-dep", edit(lambda m: p(m, "antares-model")["dependencies"].append(dep("tokio", kind="build", source="registry+x")))),
            ("ngsild-dep", edit(lambda m: p(m, "antares-model")["dependencies"].append(dep("antares-api")))),
            ("ngsild-dep", edit(lambda m: p(m, "antares-ql")["dependencies"].append(dep("antares-store")))),
            ("ngsild-dep", edit(lambda m: p(m, "antares-ql")["features"].update(default=["std", "fetch"]))),
            ("ngsild-dep", edit(lambda m: p(m, "antares-ql")["features"].update(default=["tokio"]))),
        ]
        failed = 0
        for code, run in cases:
            got = run()
            got = got if isinstance(got, list) else check(got)
            if not any(c == code for c, _ in got):
                print(f"self-test failed: a {code} violation was not reported (got {got})")
                failed += 1
        if failed:
            return 1
        print(f"module-rules self-test: OK ({len(cases)} red conditions, each reported)")
        return 0


def main():
    if sys.argv[1:] == ["--self-test"]:
        return self_test()
    if sys.argv[1:]:
        print("usage: dev/check-module-rules.py [--self-test]", file=sys.stderr)
        return 2
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--no-deps"],
        check=True, capture_output=True, text=True).stdout)
    found = check(meta)
    for code, msg in found:
        print(f"MODULE RULE {code}: {msg}")
    if found:
        return 1
    roles = sorted({antares(p).get("role") for p in meta["packages"]})
    print(f"module rules: OK ({len(meta['packages'])} packages, roles {', '.join(roles)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
