#!/usr/bin/env python3
"""Every row of bins/jc/SECURITY-REVIEW.md names the test that proves it, and each named test
exists (T-6967; the parsing follows crates/antares/dev/threat-tests.py).

A row's `Test` cell is one or more items, separated by `;` (or `,`):

  <package> `<module>::<fn>`[, `<module>::<fn>` ...]   a fn named `<fn>` under the directory of
                                                       the Cargo package `<package>`
  smoke `<suite>/<id>`                                 a check id of bins/jc/smoke/<suite>/suite.py,
                                                       written literally or as an f-string prefix
  repo:<repo> <path>                                   a test in another repository (not checked)
  none: <reason>                                       a row with no test; the reason is required

Text in parentheses is a note and never a name. A package word before a backtick span sets the
package of that span and of the spans after it, until the next package word.

`--selftest` builds a synthetic repository with one case per outcome and fails unless each is
judged as expected.
"""

import pathlib
import re
import sys
import tempfile

REVIEW = "bins/jc/SECURITY-REVIEW.md"
SMOKE = "bins/jc/smoke"
# Where Cargo packages live; target/ and node_modules/ are never searched.
PACKAGE_ROOTS = ("bins", "crates", "vendor/joinedcontext-platform/crates", "xtask")
ROW_ID = re.compile(r"^[A-Z]{1,2}-\d+$")
SPAN = re.compile(r"(?:\b([a-z][a-z0-9_-]*)\s+)?`([^`]+)`")
FSTRING = re.compile(r"""\bf(["'])(.*?)\1""")


def rows(markdown):
    """(row id, test cell or None) of every row of every table, whatever its header says (a
    renamed header never hides its rows); None when the table has no `Test` column or the cell
    is empty."""
    test_col = None
    for line in markdown.splitlines():
        if not line.startswith("|"):
            test_col = None
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split(" | ")]
        if re.fullmatch(r"[|\-: ]+", line.strip()):
            continue
        if test_col is None:  # a table's first line is its header
            test_col = cells.index("Test") if "Test" in cells else -1
            continue
        if not ROW_ID.match(cells[0]):
            continue
        yield cells[0], (cells[test_col] if 0 <= test_col < len(cells) else None)


def items(cell):
    """The cell split at `;` (and at `,` between two package words) outside parentheses."""
    out, depth, current = [], 0, []
    for char in cell:
        depth += char == "("
        depth -= char == ")"
        if char == ";" and depth <= 0:
            out.append("".join(current))
            current = []
        else:
            current.append(char)
    out.append("".join(current))
    return [item.strip() for item in out if item.strip()]


def packages(root):
    """Cargo package name -> its directory."""
    found = {}
    for base in PACKAGE_ROOTS:
        for manifest in (root / base).rglob("Cargo.toml"):
            if {"target", "node_modules"} & set(manifest.parts):
                continue
            match = re.search(r'^\[package\][^\[]*?^name\s*=\s*"([^"]+)"', manifest.read_text(errors="replace"), re.MULTILINE | re.DOTALL)
            if match:
                found.setdefault(match.group(1), manifest.parent)
    return found


def fn_exists(directory, name):
    pattern = re.compile(r"\bfn\s+" + re.escape(name) + r"\b")
    for candidate in directory.rglob("*.rs"):
        if {"target", "node_modules"} & set(candidate.parts):
            continue
        if pattern.search(candidate.read_text(errors="replace")):
            return True
    return False


def smoke_id_exists(root, suite, check_id):
    path = root / SMOKE / suite / "suite.py"
    if not path.is_file():
        return False
    text = path.read_text(errors="replace")
    if f'"{check_id}"' in text or f"'{check_id}'" in text:
        return True
    for match in FSTRING.finditer(text):
        prefix = match.group(2).split("{", 1)[0]
        if prefix and check_id.startswith(prefix):
            return True
    return False


def judge(root, known, row, cell):
    """The problems of one row."""
    if not cell:
        return [f"{row}: names no test (name one, or write `none: <reason>`)"]
    problems = []
    for item in items(cell):
        if item.startswith("none:"):
            if not item[len("none:"):].strip():
                problems.append(f"{row}: `none:` needs a reason")
            continue
        if item.startswith("repo:"):
            continue
        outside = re.sub(r"\([^()]*\)", "", item)
        package, named = None, 0
        for match in SPAN.finditer(outside):
            word, name = match.group(1), match.group(2).strip()
            if word:
                package = word
            if package is None:
                problems.append(f"{row}: `{name}` names no package")
                continue
            named += 1
            if package == "smoke":
                suite, _, check_id = name.partition("/")
                if not check_id or not smoke_id_exists(root, suite, check_id):
                    problems.append(f"{row}: smoke check {name} is not in {SMOKE}/{suite}/suite.py")
                continue
            directory = known.get(package)
            if directory is None:
                problems.append(f"{row}: package {package} does not exist")
                continue
            fn = name.split("::")[-1]
            if not re.fullmatch(r"[a-z_][a-z0-9_]*", fn) or not fn_exists(directory, fn):
                problems.append(f"{row}: fn {fn} is not in package {package}")
        if not named:
            problems.append(f"{row}: '{item}' names no test")
    return problems


def check(root, review):
    problems, count = [], 0
    known = packages(root)
    for row, cell in rows((root / review).read_text()):
        count += 1
        problems += judge(root, known, row, cell)
    if not count:
        problems.append(f"{review}: no row found (a table's first line is its header, its first cell the row id)")
    return problems, count


def selftest():
    review = """\
## 1. Fixed

| Id | Sev | What | Guard | Test |
|---|---|---|---|---|
| A-1 | high | kept | g | demo-core `guard::tests::a_test_that_exists` |
| A-2 | high | lost | g | demo-core `guard::a_test_that_was_deleted` |
| A-3 | high | two packages | g | demo-core `x::a_test_that_exists` (note `ignored`), other-pkg `t::other_test`; smoke `multiorg/iso-a-b` |
| A-4 | high | unknown package | g | ghost `t::x` |
| A-5 | high | smoke id missing | g | smoke `multiorg/not-there` |

## 2. Documented

| Id | Fix | Test |
|---|---|---|
| B-1 | f | none: latent, nothing reaches it |
| B-2 | f | none: |
| B-3 | f | repo:joinedcontext-deployment tests/test_x.py |
| B-4 | f | |
| B-5 | f | same |

| Id | Fix |
|---|---|
| C-1 | a table without a Test column names no test |

| ID | Fix |
|---|---|
| C-2 | a table headed `ID`, not `Id`, is still judged |
"""
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        files = {
            "crates/demo/core/Cargo.toml": '[package]\nname = "demo-core"\n',
            "crates/demo/core/src/guard.rs": "#[test]\nfn a_test_that_exists() {}\n",
            "crates/other/Cargo.toml": '[package]\nname = "other-pkg"\nversion = "0.1.0"\n',
            "crates/other/tests/t.rs": "#[tokio::test]\nasync fn other_test() {}\n",
            "crates/other/target/debug/x.rs": "fn a_test_that_was_deleted() {}\n",
            f"{SMOKE}/multiorg/suite.py": 'check(f"iso-{a}-{b}", ok)\ncheck("literal-id", ok)\n',
            REVIEW: review,
        }
        for name, body in files.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text(body)
        problems, count = check(root, REVIEW)
        (root / REVIEW).write_text("# A review whose tables are gone\n")
        problems += check(root, REVIEW)[0]
    expected = [
        "A-2: fn a_test_that_was_deleted is not in package demo-core",
        "A-4: package ghost does not exist",
        "A-5: smoke check multiorg/not-there is not in bins/jc/smoke/multiorg/suite.py",
        "B-2: `none:` needs a reason",
        "B-4: names no test (name one, or write `none: <reason>`)",
        "B-5: 'same' names no test",
        "C-1: names no test (name one, or write `none: <reason>`)",
        "C-2: names no test (name one, or write `none: <reason>`)",
        f"{REVIEW}: no row found (a table's first line is its header, its first cell the row id)",
    ]
    if problems != expected or count != 12:
        print(f"security-review-tests selftest FAILED: {count} rows")
        print("\n".join(problems))
        return 1
    print(f"security-review-tests selftest: {len(expected)} red cases caught, every other case passes")
    return 0


def main(argv):
    if argv == ["--selftest"]:
        return selftest()
    if argv:
        print("usage: .github/scripts/security-review-tests.py [--selftest]", file=sys.stderr)
        return 2
    root = pathlib.Path(__file__).resolve().parent.parent.parent
    problems, count = check(root, REVIEW)
    print(f"security-review-tests: {count} rows of {REVIEW}")
    for problem in problems:
        print(f"  MISSING  {problem}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
