"""The Portal walker's DOM checks, taken from the Portal's own source at run time.

crates/portal/ui/tests/pageChecks.ts writes `pageFindings`, `viewerWriteFindings` and
`blankDocument` to be handed to `page.evaluate` as they are (no imports, no closure). This module
cuts those three functions out of the file and strips their TypeScript annotations, so the smoke
suite judges a page with the same rules as the live walker and the fast vitest, with no copy that
can drift. `tests/test_ui.py` evaluates the result in Chromium against fixture pages.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
UI = ROOT / "crates" / "portal" / "ui"
PAGE_CHECKS = UI / "tests" / "pageChecks.ts"
EN_JSON = UI / "src" / "locales" / "en.json"
ALLOW = UI / "e2e" / "live" / "walker.allow.json"

FUNCTIONS = ("pageFindings", "viewerWriteFindings", "blankDocument")

# Only the type names pageChecks.ts uses, so a ternary's `: value` is never touched.
_TYPES = r"(?:PageCheckOptions|Element \| null|Element|string\[\]|string|boolean|number)"
_STRIP = [
    (re.compile(r"\s+as\s+HTML[A-Za-z]*Element\b"), ""),
    (re.compile(r"new (Set|Map)<[^>()]*>\("), r"new \1("),
    (re.compile(r"(\w)\s*:\s*" + _TYPES + r"(?=\s*[,)=])"), r"\1"),
    (re.compile(r"\)\s*:\s*" + _TYPES + r"\s*\{"), ") {"),
]


def _function(src: str, name: str) -> str:
    start = src.index(f"export function {name}(")
    open_at = src.index("{", src.index(")", start))
    # The parameter list may hold `): string[] {`: the body starts at the first `{` after `)`.
    depth, i = 0, open_at
    while True:
        c = src[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return src[start + len("export "):i + 1]
        elif c in "'\"`":
            i = _skip_string(src, i)
        elif src.startswith("//", i):
            i = src.index("\n", i)
        elif src.startswith("/*", i):
            i = src.index("*/", i) + 1
        elif c == "/" and _regex_start(src, i):
            i = _skip_regex(src, i)
        i += 1


def _skip_string(src: str, i: int) -> int:
    q, i = src[i], i + 1
    while src[i] != q:
        i += 2 if src[i] == "\\" else 1
    return i


def _regex_start(src: str, i: int) -> bool:
    before = src[:i].rstrip()
    return not before or before[-1] in "(,=:[!&|?{};" or before.endswith("return")


def _skip_regex(src: str, i: int) -> int:
    i += 1
    in_class = False
    while True:
        c = src[i]
        if c == "\\":
            i += 2
            continue
        if c == "[":
            in_class = True
        elif c == "]":
            in_class = False
        elif c == "/" and not in_class:
            return i
        i += 1


def strip_types(ts: str) -> str:
    for pattern, repl in _STRIP:
        ts = pattern.sub(repl, ts)
    return ts


def script() -> str:
    """One browser script defining the three functions on `window.__jcSmoke`."""
    src = PAGE_CHECKS.read_text()
    body = "\n".join(strip_types(_function(src, n)) for n in FUNCTIONS)
    return f"(() => {{ {body}\nwindow.__jcSmoke = {{ {', '.join(FUNCTIONS)} }}; }})();"


def namespaces() -> list[str]:
    return sorted(json.loads(EN_JSON.read_text()).keys())


def excused() -> list[dict]:
    try:
        return json.loads(ALLOW.read_text()).get("excused", [])
    except (OSError, ValueError):
        return []


def excuse(route: str, finding: str, allow: list[dict]) -> dict | None:
    """The allow-list entry (route, check, task) that excuses a finding, as `unexcused` reads it."""
    return next((e for e in allow if e.get("route") == route and finding.startswith(f"{e.get('check')}:")), None)


# What the smoke suite adds beside pageChecks.ts (owner 2026-10-20: no URNs in tables): a cell of
# a table whose whole text is a URN, unless its column names an id. Self-contained for evaluate.
TABLE_URN = r"""() => {
  const found = [];
  for (const table of document.querySelectorAll("main table")) {
    const heads = [...table.querySelectorAll(":scope > thead th, :scope > thead td")].map((h) => (h.textContent || "").trim());
    for (const row of table.querySelectorAll(":scope > tbody > tr")) {
      [...row.children].forEach((cell, i) => {
        const value = (cell.textContent || "").replace(/\s+/g, " ").trim();
        if (!/^urn:ngsi-ld:\S+$/.test(value) || cell.closest("pre, code")) return;
        if (/\b(id|ids|identifier|entity)\b/i.test(heads[i] || "")) return;
        found.push(`table-urn: ${heads[i] || "column " + (i + 1)}: ${value.slice(0, 120)}`);
      });
    }
  }
  return [...new Set(found)].slice(0, 5);
}"""
