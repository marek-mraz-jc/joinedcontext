#!/usr/bin/env python3
"""Every STRIDE row of the surfaces threat model names a test that exists
(surfaces-test-plan.md section 8, T-3492). The model is docs/analysis/surfaces-threat-model.md
plus one file per surface under docs/analysis/threat-model/ (T-3766), each keeping its
`## <section>.` heading, so a row's section is the same wherever it lives.

A row's test column names its tests in backticks, one item per `;`:

  `file.rs::fn` or `file.rs::module::fn`      a fn in a file of that name under crates/
  `crates/<c>/tests/file.rs::fn`               the same, at exactly that path
  `bare_fn_name`                               a fn of that name anywhere under crates/
  `dev/<script> --self-test`, `crates/<c>/...` a file at that path (the first word)
  fuzz target `fuzz/fuzz_targets/x.rs` or `x`  the target file under fuzz/fuzz_targets/

The first backtick span of an item is a name, and so is every span that continues a comma list
after it (`a`, `b`, `c`); a span after other words ("doc test `Ops` with a `compile_fail`
block") is prose.

When a name is checked:
  - an item marked `(proposed, T-nnnn` is owned by that task, whose crate may already exist while
    the task is still open (a crate is built in rounds): it waits until its test exists, and
    once the test exists the tag is a problem until it is dropped, so the row then guards the
    test like any other and a later deletion is caught;
  - an item marked `(proposed` is owned by a crate that may not exist yet: its path's crate when
    that is one of the new crates of ADR-0027 section 1, else the crate of the row's section
    (2 antares-ops, 3 antares-views, 4 antares-odata, 5 antares-mcp, 6 antares-linkml,
    7 antares-ws, 8 antares-icsr). It is checked once crates/<owner>/Cargo.toml exists;
  - a `file.rs::fn` item not marked proposed names a test that exists today, so it is checked
    now: a threat cannot lose its test silently;
  - any other unmarked item (a conformance file, a script) is owned by the section's crate.

A STRIDE row that names nothing at all fails too. `--self-test` builds a synthetic repository
with one case per outcome and fails unless each is judged as expected.
"""

import pathlib
import re
import sys
import tempfile

MODEL = "docs/analysis/surfaces-threat-model.md"
SURFACES = "docs/analysis/threat-model"
SECTION_CRATE = {
    2: "antares-ops",
    3: "antares-views",
    4: "antares-odata",
    5: "antares-mcp",
    6: "antares-linkml",
    7: "antares-ws",
    8: "antares-icsr",
}
# ADR-0027 section 1: the crates the surfaces epic adds.
NEW_CRATES = set(SECTION_CRATE.values()) | {
    "odata-url",
    "odata-csdl",
    "odata-json",
    "odata-batch",
    "odata-axum",
    "linkml-model",
    "antares-residual",
}
NAME = re.compile(r"^[a-z_][a-z0-9_]*$")
TASK_TAG = re.compile(r"\(proposed,\s*(T-\d+)\b")


def top_level_items(cell):
    """The cell split at `;` outside parentheses."""
    items, depth, current = [], 0, []
    for char in cell:
        depth += char == "("
        depth -= char == ")"
        if char == ";" and depth <= 0:
            items.append("".join(current))
            current = []
        else:
            current.append(char)
    items.append("".join(current))
    return [item.strip() for item in items if item.strip()]


def names_of(item):
    """The backtick spans of an item that name something (see the module doc)."""
    outside = re.sub(r"\([^()]*\)", "", item)
    names = []
    for match in re.finditer(r"`([^`]+)`", outside):
        before = outside[: match.start()]
        if names and not re.search(r",\s*$", before):
            break
        names.append(match.group(1))
    return names


def rows(markdown):
    """(section, row id, test cell) of every STRIDE row in sections 2 to 8."""
    section, stride = None, False
    for line in markdown.splitlines():
        heading = re.match(r"^## (\d+)\.", line)
        if heading:
            section, stride = int(heading.group(1)), False
            continue
        if not line.startswith("|"):
            stride = False
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split(" | ")]
        if cells[:2] == ["id", "S/T/R/I/D/E"]:
            stride = True
            continue
        if stride and section in SECTION_CRATE and not re.fullmatch(r"[|\-: ]+", line.strip()):
            yield section, cells[0], cells[-1]


def model_text(root):
    """The index, then every surface file in name order."""
    parts = [(root / MODEL).read_text()]
    parts += [f.read_text() for f in sorted((root / SURFACES).glob("*.md"))]
    return "\n".join(parts)


def crate_of_path(path):
    match = re.match(r"crates/([^/]+)/", path)
    return match.group(1) if match else None


def fn_exists(root, name, file=None, path=None):
    """Whether `fn name` is defined in a .rs file under crates/ (one file name, or one path)."""
    pattern = re.compile(r"\bfn\s+" + re.escape(name) + r"\b")
    if path:
        candidates = [root / path]
    else:
        candidates = (root / "crates").rglob(file or "*.rs")
    for candidate in candidates:
        if candidate.is_file() and pattern.search(candidate.read_text(errors="replace")):
            return True
    return False


def judge(root, section, item, name):
    """(owner crate or None for "checked now", what is missing or None)."""
    proposed = "(proposed" in item
    fuzz = "fuzz target" in item
    words = name.split()
    target = words[0] if words else name
    path_crate = crate_of_path(target)
    if proposed:
        owner = path_crate if path_crate in NEW_CRATES else SECTION_CRATE[section]
    elif ".rs::" in target and not fuzz:
        owner = None
    else:
        owner = SECTION_CRATE[section]
    if owner and not (root / "crates" / owner / "Cargo.toml").is_file():
        return owner, None
    if fuzz or target.startswith("fuzz/"):
        file = target if "/" in target else f"fuzz/fuzz_targets/{target}.rs"
        return owner, None if (root / file).is_file() else f"fuzz target {file}"
    if ".rs::" in target:
        file, _, rest = target.partition("::")
        fn = rest.split("::")[-1]
        found = (
            fn_exists(root, fn, path=file)
            if "/" in file
            else fn_exists(root, fn, file=file)
        )
        return owner, None if found else f"fn {fn} in {file}"
    if NAME.match(target):
        return owner, None if fn_exists(root, target) else f"fn {target}"
    if "/" in target:
        return owner, None if (root / target).exists() else f"file {target}"
    # A bare file name (a conformance artefact): anywhere in the repository but target/.
    found = any(
        "target" not in hit.parts for hit in root.rglob(target) if hit.is_file()
    )
    return owner, None if found else f"file {target}"


def check(root):
    """(problems, checked, pending, rows) over the threat model under `root`."""
    problems, checked, pending, count = [], 0, 0, 0
    for section, row, cell in rows(model_text(root)):
        count += 1
        named = [(item, name) for item in top_level_items(cell) for name in names_of(item)]
        if not named:
            problems.append(f"{row}: names no test")
        for item, name in named:
            owner, missing = judge(root, section, item, name)
            if owner and not (root / "crates" / owner / "Cargo.toml").is_file():
                pending += 1
                continue
            tag = TASK_TAG.search(item)
            if tag and missing:
                pending += 1
                continue
            if tag:
                checked += 1
                problems.append(f"{row}: {name} exists; drop the {tag.group(1)} tag so the row guards it")
                continue
            checked += 1
            if missing:
                why = f"crate {owner} exists" if owner else "named as an existing test"
                problems.append(f"{row}: {missing} does not exist ({why})")
    return problems, checked, pending, count


def self_test():
    model = """\
## 1. Scope

| id | S/T/R/I/D/E | threat | mitigation | test |
|---|---|---|---|---|
| Z1 | I | outside sections 2 to 8 | none | `never_checked` |

## 2. Shared

| # | boundary | who | what |
|---|---|---|---|
| B0 | not a STRIDE row | x | `ignored_name` |

| id | S/T/R/I/D/E | threat | mitigation | test |
|---|---|---|---|---|
| G1 | E | today's test kept | m | `seam.rs::a_test_that_exists` |
| G2 | E | today's test lost | m | `seam.rs::a_test_that_was_deleted` |
| G3 | E | proposed in a missing crate | m | `ops_seam.rs::not_written_yet` (proposed) |
| G4 | E | prose after a path | m | `crates/antares-broker/src/lib.rs` doc test `Ops` with a `compile_fail` block (proposed) |
| G5 | E | names nothing | m | a conftest rule elsewhere (proposed, outside this repository) |

## 3. Views

In threat-model/views.md.
"""
    views = """\
# Threat model: Views

## 3. Views

| id | S/T/R/I/D/E | threat | mitigation | test |
|---|---|---|---|---|
| V1 | I | proposed, crate exists, written | m | `views_present` (proposed, ADR-0023) |
| V2 | I | proposed, crate exists, a comma list with one missing | m | `views_present`, `views_absent` (proposed; the last asserts 400); `seam.rs::a_test_that_exists` |
| V3 | I | fuzz target missing | m | fuzz target `fuzz/fuzz_targets/view_load.rs` (proposed) |
| V4 | I | script present | m | `dev/check.py --self-test` (proposed) |
| V5 | I | script missing | m | `dev/missing.sh --self-test` (proposed) |
| V6 | I | path crate is an existing core crate | m | `crates/antares-broker/tests/fail_closed.rs::closed` (proposed) |
| V7 | I | owned by an open task, not written yet | m | `views_writes.rs::later` (proposed, T-0001) |
| V8 | I | owned by a task, written, tag left | m | `views.rs::views_present` (proposed, T-0002: a note) |
"""
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "docs/analysis").mkdir(parents=True)
        (root / MODEL).write_text(model)
        (root / SURFACES).mkdir()
        (root / SURFACES / "views.md").write_text(views)
        for crate, files in {
            "antares-broker": {"tests/seam.rs": "fn a_test_that_exists() {}", "src/lib.rs": ""},
            "antares-views": {"tests/views.rs": "#[test]\nfn views_present() {}"},
        }.items():
            (root / "crates" / crate).mkdir(parents=True)
            (root / "crates" / crate / "Cargo.toml").write_text("")
            for name, body in files.items():
                (root / "crates" / crate / name).parent.mkdir(parents=True, exist_ok=True)
                (root / "crates" / crate / name).write_text(body)
        (root / "dev").mkdir()
        (root / "dev/check.py").write_text("")
        problems, checked, pending, count = check(root)
    expected = [
        "G2: fn a_test_that_was_deleted in seam.rs does not exist (named as an existing test)",
        "G5: names no test",
        "V2: fn views_absent does not exist (crate antares-views exists)",
        "V3: fuzz target fuzz/fuzz_targets/view_load.rs does not exist (crate antares-views exists)",
        "V5: file dev/missing.sh does not exist (crate antares-views exists)",
        "V6: fn closed in crates/antares-broker/tests/fail_closed.rs does not exist (crate antares-views exists)",
        "V8: views.rs::views_present exists; drop the T-0002 tag so the row guards it",
    ]
    ok = problems == expected and count == 13 and pending == 3 and checked == 11
    if not ok:
        print(f"threat-tests self-test FAILED: rows {count}, checked {checked}, pending {pending}")
        print("\n".join(problems))
        return 1
    print(f"threat-tests self-test: {len(expected)} red cases caught, every other case passes")
    return 0


def main(argv):
    if argv == ["--self-test"]:
        return self_test()
    root = pathlib.Path(__file__).resolve().parent.parent
    if argv == ["--ids"]:
        # one line per row, to compare the model before and after a move
        for section, row, cell in rows(model_text(root)):
            print(section, row, cell)
        return 0
    if argv:
        print("usage: dev/threat-tests.py [--self-test | --ids]", file=sys.stderr)
        return 2
    problems, checked, pending, count = check(root)
    print(
        f"threat-tests: {count} STRIDE rows, {checked} named tests checked, "
        f"{pending} waiting for their crate or task"
    )
    for problem in problems:
        print(f"  MISSING  {problem}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
