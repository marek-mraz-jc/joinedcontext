#!/usr/bin/env python3
"""Code no production entry point reaches, the Rust crate and the UI apart, as a ratchet
(T-4454, after joinedcontext-platform's dev/cg-dead-code.py, T-3918).

Entry points: the binaries' `main` (`src/main.rs`, `src/bin/*.rs`, `build.rs`), from which the
router reaches every route handler; the UI's `ui/src/main.tsx`; and every module the UI imports
lazily (`import("./pages/…")`), which the graph has no edge for. A symbol is reached through
CALLS, DISPATCHES, REFERENCES and IMPORTS from an entry point. Every file a `ui/vite*.config.ts`
names (a library `entry`, an alias `replacement`) is an entry point too: the explorer widget's
`mount` is called by the page that loads its script, and its stand-in modules by the bundle. A Rust type reached also reaches
its trait impls (derives, `From` through `?`, `Display` in `format!`) and its fields and
variants; a UI symbol reached also reaches its file's module, whose top-level code runs on
import. What is left outside tests is a row: a function, method, type, constant or class.

ci/cg-dead-code-baseline.txt holds `package | rows`. A package with more rows than its line is
red (new code nothing calls, or a caller deleted and its callees kept); one with fewer is red
too until the line is lowered, so the baseline only goes down.

  dev/cg-dead-code.py                    query the graph (`cg build --fg` first) and check
  dev/cg-dead-code.py --rows             also print every row, by file
  dev/cg-dead-code.py --from DIR         read DIR/symbols.txt, edges.txt, members.txt (saved `cg q`)
  dev/cg-dead-code.py --selftest

A function a serde attribute names by string (`default = "f"`, `skip_serializing_if = "f"`,
`deserialize_with`, `serialize_with`) is reached, and so is a constant a format string
captures (`"{PAGE_LIMIT}"`): cg has no edge for a string. So is a `#[global_allocator]` static,
which only the compiler uses. A `macro_rules!` body or a closure
stored in a field stays a row; GREP each row before a delete.
"""

from __future__ import annotations

import collections
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
BASELINE = ROOT / "ci" / "cg-dead-code-baseline.txt"
RUST, UI = "joinedcontext-portal", "portal-ui"
SCOPE = f"['{RUST}','{UI}']"
SYMBOLS = (
    f"MATCH (s:Symbol) WHERE s.package IN {SCOPE} AND NOT s.external RETURN s.id, s.path, "
    "s.kind, s.package, s.file, s.line, s.loc, s.visibility, s.is_test, s.trait_name, s.self_type, "
    "s.cfg;"
)
EDGES = (
    "MATCH (a:Symbol)-[r:CALLS|DISPATCHES|REFERENCES|IMPORTS]->(b:Symbol) "
    f"WHERE a.package IN {SCOPE} AND b.package IN {SCOPE} RETURN a.id, b.id;"
)
# A type reaches its fields and variants, and through them the types they hold.
MEMBERS = (
    f"MATCH (a:Symbol)-[r:CONTAINS|HAS_METHOD]->(b:Symbol) WHERE a.package IN {SCOPE} "
    "AND (label(r) = 'CONTAINS' AND a.kind IN ['Struct','Enum'] OR label(r) = 'HAS_METHOD' AND a.kind = 'Class') "
    "RETURN a.id, b.id;"
)
KINDS = {"Function", "Method", "StaticMethod", "TraitMethod", "Struct", "Enum", "Constant",
         "StaticVariable", "TypeAlias", "Trait", "Macro", "Class"}
SERDE_FN = re.compile(
    r'\b(?:default|skip_serializing_if|deserialize_with|serialize_with)\s*=\s*"(?:[\w:]*?(\w+)::)?(\w+)(?:::<[^"]*>)?"')
CAPTURE = re.compile(r'\{([A-Z][A-Z0-9_]*)[}:]')
ALLOCATOR = re.compile(r'#\[global_allocator\]\s*(?:pub(?:\([^)]*\))?\s+)?static\s+(\w+)')
# `import("./pages/X").then((module) => ({ default: module.X }))`: the module, and the name taken.
LAZY = re.compile(r'\bimport\(\s*["\'](\.[^"\']+)["\']\s*\)'
                  r'(?:\.then\(\s*\(?(\w+)\)?\s*=>\s*\(\{\s*default:\s*\2\.(\w+))?')
# A static import of a module: its top level runs, whatever the graph resolved (a default import
# of a value has no edge in it).
DEFAULT = re.compile(r'\bexport\s+default\s+(?:async\s+)?(?:function\s+|class\s+)?(\w+)')
STATIC = re.compile(r'\bfrom\s+["\'](\.[^"\']+)["\']')
UI_ENTRY = "ui/src/main.tsx"
# `entry: "src/explorer/main.tsx"`, `new URL("./src/explorer/i18n.ts", import.meta.url)`
VITE_FILE = re.compile(r'["\'](?:\./)?(src/[^"\']+\.tsx?)["\']')
UI_TEST = re.compile(r'(\.test\.|\.spec\.|/test/|/__tests__/|/setupTests|/test-setup|/api/schema\.d\.ts$)')


def table(text: str) -> list[dict[str, str]]:
    """The rows of one `cg q` table; problems (a hint, a truncation) raise."""
    lines = text.splitlines()
    for line in lines:
        if line.startswith("hint:") or "output stops at" in line:
            raise ValueError(line)
    start = next((i for i, line in enumerate(lines) if line.startswith("### Q")), -1)
    head = next(i for i, line in enumerate(lines) if i > start and " | " in line)
    cols = [c.strip() for c in lines[head].split(" | ")]
    return [dict(zip(cols, (c.strip() for c in line.split(" | "))))
            for line in lines[head + 1:] if line.count(" | ") == len(cols) - 1]


def production(s: dict[str, str]) -> bool:
    f = s["s.file"]
    # `#[cfg(test)]` on the item or a module around it is test code the graph does not mark
    return (s["s.is_test"] == "False" and "test" not in s.get("s.cfg", "") and s["s.kind"] in KINDS
            and (f.startswith("src/") or (f.startswith("ui/src/") and not UI_TEST.search(f))))


def is_main(s: dict[str, str]) -> bool:
    f = s["s.file"]
    return (s["s.kind"] == "Function" and s["s.path"].endswith("::main")
            and (f in ("src/main.rs", "build.rs") or f.startswith("src/bin/")))


def string_named(root: pathlib.Path) -> set[tuple[str, str]]:
    """What a string names, which cg has no edge for: a serde attribute's function, keyed
    `(file, fn)` when bare and `("", Type::fn)` when it names a type, and a constant a format
    string captures, keyed `("", NAME)`; a `#[global_allocator]` static, keyed `(file, NAME)`."""
    found = set()
    for p in root.glob("src/**/*.rs"):
        text, rel = p.read_text(errors="ignore"), str(p.relative_to(root))
        for owner, name in SERDE_FN.findall(text):
            found.add(("", f"{owner}::{name}") if owner else (rel, name))
        found |= {("", name) for name in CAPTURE.findall(text)}
        found |= {(rel, name) for name in ALLOCATOR.findall(text)}
    return found


def resolve(root: pathlib.Path, importer: pathlib.Path, spec: str) -> str | None:
    base = (importer.parent / spec).resolve()
    for cand in (base, *(base.with_name(base.name + e) for e in (".tsx", ".ts")),
                 base / "index.tsx", base / "index.ts"):
        if cand.is_file() and cand.suffix in (".ts", ".tsx"):
            return str(cand.relative_to(root))
    return None


def ui_imports(root: pathlib.Path) -> tuple[set[tuple[str, str]], set[tuple[str, str]]]:
    """The UI's imports the graph may miss, by text: `(importer, module)` for every relative
    static import, and `(module, name)` for every lazy one (the name a `.then` picks, else the
    module's default export)."""
    static, lazy = set(), set()
    for p in (root / "ui" / "src").rglob("*.ts*"):
        rel = str(p.relative_to(root))
        if UI_TEST.search(rel):
            continue
        text = p.read_text(errors="ignore")
        for spec in STATIC.findall(text):
            if (target := resolve(root, p, spec)):
                static.add((rel, target))
        for spec, _, name in LAZY.findall(text):
            if (target := resolve(root, p, spec)):
                if not name:
                    found = DEFAULT.search((root / target).read_text(errors="ignore"))
                    name = found.group(1) if found else ""
                lazy.add((target, name))
    return static, lazy


def vite_entries(root: pathlib.Path) -> set[str]:
    """The UI files the Vite configs name, each a whole-file entry point."""
    return {f"ui/{f}" for p in (root / "ui").glob("vite*.config.ts")
            for f in VITE_FILE.findall(p.read_text(errors="ignore")) if (root / "ui" / f).is_file()}


def unreachable(symbols: list[dict[str, str]], edges: list[dict[str, str]],
                named: set[tuple[str, str]] = frozenset(),
                imports: tuple[set[tuple[str, str]], set[tuple[str, str]]] = (set(), set()),
                ui_entries: frozenset[str] | set[str] = frozenset({UI_ENTRY})
                ) -> list[dict[str, str]]:
    syms = {s["s.id"]: s for s in symbols}
    out = collections.defaultdict(set)
    for e in edges:
        out[e["a.id"]].add(e["b.id"])
    impls = collections.defaultdict(set)
    module, classes = {}, {}
    for i, s in syms.items():
        if s["s.package"] == RUST and s["s.trait_name"] and s["s.self_type"]:
            # by the bare name: `impl<'a> VisitMut<'a> for Imports<'a, '_>` is Imports's
            impls[s["s.self_type"].split("<", 1)[0]].add(i)
        if s["s.package"] == UI and s["s.kind"] == "Module":
            module[s["s.file"]] = i
        if s["s.package"] == UI and s["s.kind"] == "Class":
            classes[(s["s.file"], s["s.path"].rsplit("::", 1)[-1])] = i
    static, lazy = imports
    for importer, target in static:
        if importer in module and target in module:
            out[module[importer]].add(module[target])
    lazy_names = {(f, n) for f, n in lazy if n}
    entries = {module[f] for f, _ in lazy if f in module}
    for i, s in syms.items():
        name = s["s.path"].rsplit("::", 1)[-1]
        owned = f"{s['s.self_type']}::{name}" if s["s.self_type"] else name
        if is_main(s) or s["s.file"] in ui_entries or (s["s.file"], name) in lazy_names:
            entries.add(i)
        elif production(s) and s["s.package"] == RUST and (
                (s["s.file"], name) in named or ("", owned) in named or ("", name) in named
                and s["s.kind"] in ("Constant", "StaticVariable")):
            entries.add(i)
    seen, stack = set(entries), list(entries)
    while stack:
        x = stack.pop()
        s = syms.get(x)
        nxt = set(out.get(x, ()))
        if s and s["s.kind"] in ("Struct", "Enum", "TypeAlias") and s["s.package"] == RUST:
            nxt |= impls.get(s["s.path"].rsplit("::", 1)[-1], set())
        if s and s["s.package"] == UI and s["s.file"] in module:
            nxt.add(module[s["s.file"]])
        if s and s["s.package"] == UI and (s["s.file"], s["s.self_type"]) in classes:
            nxt.add(classes[(s["s.file"], s["s.self_type"])])  # `new C()` lands on the constructor
        for y in nxt - seen:
            seen.add(y)
            stack.append(y)
    rows = [s for i, s in syms.items() if i not in seen and production(s) and not is_main(s)]
    return sorted(rows, key=lambda s: (s["s.file"], int(s["s.line"] or 0)))


def baseline(text: str) -> dict[str, int]:
    out = {}
    for line in text.splitlines():
        if line.strip() and not line.startswith("#"):
            name, count = (p.strip() for p in line.split("|"))
            out[name] = int(count)
    return out


def check(rows: list[dict[str, str]], allowed: dict[str, int]) -> list[str]:
    counts = collections.Counter(s["s.package"] for s in rows)
    problems = []
    for name in sorted(set(counts) | set(allowed)):
        have, may = counts.get(name, 0), allowed.get(name, 0)
        if have > may:
            problems.append(f"{name}: {have} unreachable, baseline {may}; new rows:")
            problems += [f"  {s['s.file']}:{s['s.line']} {s['s.kind']} {s['s.path']}"
                         for s in rows if s["s.package"] == name][:40]
        elif have < may:
            problems.append(f"{name}: {have} unreachable, baseline {may}: lower its line in "
                            f"{BASELINE.relative_to(ROOT)}")
    return problems


def selftest() -> None:
    def sym(i, path, kind="Function", pkg=RUST, file="src/lib.rs", trait="", self_type=""):
        return {"s.id": i, "s.path": path, "s.kind": kind, "s.package": pkg, "s.file": file,
                "s.line": "1", "s.loc": "3", "s.visibility": "pub", "s.is_test": "False",
                "s.trait_name": trait, "s.self_type": self_type}
    page = "ui/src/pages/Lazy.tsx"
    symbols = [
        sym("m", "p::main", file="src/main.rs"),
        sym("h", "p::api::handler", file="src/api/x.rs"),
        sym("t", "p::Thing", kind="Struct"),
        sym("f", "p::Thing::fmt", kind="Method", trait="Display", self_type="Thing"),
        sym("x", "p::cg_fixture_unreachable"),
        sym("d", "p::default_mode"),
        sym("c", "p::LIMIT", kind="Constant", file="src/page.rs"),
        sym("g", "p::ALLOCATOR", kind="StaticVariable", file="src/main.rs"),
        sym("z", "p::helper", file="tests/t.rs") | {"s.is_test": "True"},
        sym("ct", "p::for_unit_tests") | {"s.cfg": "test"},
        sym("um", UI_ENTRY, kind="Module", pkg=UI, file=UI_ENTRY),
        sym("ua", "ui/src/App.tsx::App", pkg=UI, file="ui/src/App.tsx"),
        sym("uM", "ui/src/App.tsx", kind="Module", pkg=UI, file="ui/src/App.tsx"),
        sym("ub", "ui/src/App.tsx::banner", pkg=UI, file="ui/src/App.tsx"),
        sym("ul", f"{page}::Lazy", pkg=UI, file=page),
        sym("uh", f"{page}::unused", pkg=UI, file=page),
        sym("vM", "ui/src/v.ts", kind="Module", pkg=UI, file="ui/src/v.ts"),
        sym("vc", "ui/src/v.ts::V", kind="Class", pkg=UI, file="ui/src/v.ts"),
        sym("vm", "ui/src/v.ts::V::render", kind="Method", pkg=UI, file="ui/src/v.ts"),
        sym("ec", "ui/src/v.ts::E", kind="Class", pkg=UI, file="ui/src/v.ts"),
        sym("en", "ui/src/v.ts::E::<constructor>", kind="Constructor", pkg=UI, file="ui/src/v.ts",
            self_type="E"),
        sym("w", "p::Walker", kind="Struct"),
        sym("wv", "p::Walker::visit_x", kind="Method", trait="VisitMut<'a>", self_type="Walker<'a>"),
        sym("ux", "ui/src/util.ts::cgFixtureUnreachable", pkg=UI, file="ui/src/util.ts"),
        sym("ut", "ui/src/util.test.ts::helper", pkg=UI, file="ui/src/util.test.ts"),
        sym("wm", "ui/src/widget/main.tsx::mount", pkg=UI, file="ui/src/widget/main.tsx"),
    ]
    # main -> handler -> Thing; main.tsx -> App; App's module top level calls banner; App.tsx
    # imports v.ts by default (no edge), whose top level makes a V, whose render React calls
    edges = [{"a.id": "m", "b.id": "h"}, {"a.id": "h", "b.id": "t"},
             {"a.id": "um", "b.id": "ua"}, {"a.id": "uM", "b.id": "ub"},
             {"a.id": "vM", "b.id": "vc"}, {"a.id": "vc", "b.id": "vm"},
             {"a.id": "vm", "b.id": "en"}, {"a.id": "h", "b.id": "w"}]
    rows = unreachable(symbols, edges, {("src/lib.rs", "default_mode"), ("", "LIMIT"),
                                        ("src/main.rs", "ALLOCATOR")},
                       ({("ui/src/App.tsx", "ui/src/v.ts")}, {(page, "Lazy")}),
                       {UI_ENTRY, "ui/src/widget/main.tsx"})
    assert VITE_FILE.findall('entry: "src/explorer/main.tsx", x: new URL("./src/no-map.ts", u), '
                             'y: new URL("./src/no-map.css", u)') == ["src/explorer/main.tsx", "src/no-map.ts"]
    assert SERDE_FN.findall('#[serde(default = "default_mode", skip_serializing_if = "x::is_unset")]') \
        == [("", "default_mode"), ("x", "is_unset")]
    assert CAPTURE.findall('format!("limit={LIMIT}&w={WIDTH:>4}", x = "{lower}")') == ["LIMIT", "WIDTH"]
    assert LAZY.findall('lazy(() => import("./pages/L").then((module) => ({ default: module.L })))') \
        == [("./pages/L", "module", "L")]
    assert LAZY.findall('() => import("./locales/sk.json")') == [("./locales/sk.json", "", "")]
    assert ALLOCATOR.findall("#[global_allocator]\nstatic ALLOCATOR: Jemalloc = Jemalloc;") == ["ALLOCATOR"]
    assert DEFAULT.findall("export default function MonacoSourceView(") == ["MonacoSourceView"]
    assert STATIC.findall('import validator from "./forms/validator";') == ["./forms/validator"]
    assert sorted(s["s.path"] for s in rows) == ["p::cg_fixture_unreachable",
                                                f"{page}::unused",
                                                "ui/src/util.ts::cgFixtureUnreachable"], rows
    assert check(rows, {RUST: 1, UI: 2}) == []
    grown = check(rows, {RUST: 0, UI: 2})
    assert grown[0].startswith(f"{RUST}: 1 unreachable, baseline 0") and \
        "cg_fixture_unreachable" in grown[1], grown
    assert "lower its line" in check(rows, {RUST: 2, UI: 2})[0]
    assert table("graph: built | STALE: a.rs\n### Q1\na.id | b.id\n1 | 2\n") == [{"a.id": "1", "b.id": "2"}]
    try:
        table("hint: 'x' matches no Symbol\n")
        raise AssertionError("a hint must fail")
    except ValueError:
        pass
    print("cg-dead-code selftest: ok")


def query(statement: str) -> str:
    run = subprocess.run(["cg", "q", "--max-rows", "1000000", "--timeout", "300", "-e",
                          statement], cwd=ROOT, capture_output=True, text=True, check=False)
    if run.returncode != 0:
        raise SystemExit(run.stdout + run.stderr)
    return run.stdout


def main(argv: list[str]) -> int:
    args = argv[1:]
    if args == ["--selftest"]:
        selftest()
        return 0
    if "--from" in args:
        d = pathlib.Path(args[args.index("--from") + 1])
        texts = [(d / f).read_text() for f in ("symbols.txt", "edges.txt", "members.txt")]
    else:
        texts = [query(q) for q in (SYMBOLS, EDGES, MEMBERS)]
    sym_text, edge_text, member_text = texts
    rows = unreachable(table(sym_text), table(edge_text) + table(member_text),
                       string_named(ROOT), ui_imports(ROOT), {UI_ENTRY} | vite_entries(ROOT))
    counts = collections.Counter(s["s.package"] for s in rows)
    for name in sorted(counts):
        print(f"{name} | {counts[name]}")
    if "--rows" in args:
        for s in rows:
            print(f"{s['s.file']}:{s['s.line']} | {s['s.kind']} | {s['s.path']} | {s['s.loc']}")
    problems = check(rows, baseline(BASELINE.read_text()) if BASELINE.is_file() else {})
    for p in problems:
        print(p)
    print(f"cg-dead-code: {len(problems)} problem(s)" if problems else "cg-dead-code: OK")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
