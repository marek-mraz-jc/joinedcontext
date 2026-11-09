"""The name diff: production's names of one kind against jc's, with the listed renames.

A prod name jc lacks fails, unless RENAMES.md lists it (kind, old = the prod name) and both hold:
jc has the new name, and the old address still answers on jc. A jc name prod lacks is a report
line (`extra`), never a pass for a missing one: a missing name's row names the extras of its
scope as what may stand in for it, so the rename gets written down instead. A row of kind
`<kind> (extra)` with old `(none)` writes down a jc name prod never held (RENAMES.md "dataset
(extra)"): a suite that counts jc's names accepts that name as an extra, and no other."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from common import SKIP

KINDS = ("project", "space", "endpoint-name", "endpoint", "surface", "mcp-server", "tool", "dataset",
         "app", "dashboard", "realm", "client", "connector")


@dataclass
class Rename:
    kind: str
    old: str
    new: str
    why: str
    keeps: str  # how old clients keep working
    extra: bool = False  # a `<kind> (extra)` row: `new` is a jc name prod never held, `old` is "(none)"


def load_renames(path: str | None) -> list[Rename]:
    """The table rows of RENAMES.md: | kind | old | new | why | how old clients keep working |.
    No file means no renames. A row naming an unknown kind or missing a cell is refused, and so is
    an extra row (`<kind> (extra)`) whose old cell is not `(none)`."""
    if not path:
        return []
    out = []
    for n, line in enumerate(Path(path).read_text().splitlines(), 1):
        line = line.strip()
        if not line.startswith("|"):
            continue
        cells = [c.strip().strip("`") for c in line.strip("|").split("|")]
        if not cells or cells[0].lower() == "kind" or re.fullmatch(r":?-+:?", cells[0]):
            continue
        if len(cells) < 5 or not all(cells[:5]):
            raise SystemExit(f"{path}:{n}: a rename needs kind, old, new, why and how old clients keep working")
        kind, extra = re.subn(r"\s*\(extra\)$", "", cells[0])
        if kind not in KINDS:
            raise SystemExit(f"{path}:{n}: unknown kind {cells[0]!r}; kinds: {', '.join(KINDS)}")
        if extra and cells[1] != "(none)":
            raise SystemExit(f"{path}:{n}: an extra names a jc name prod never held: its old cell is (none)")
        out.append(Rename(kind, *cells[1:5], extra=bool(extra)))
    return out


def scope(name: str) -> str:
    """What a name shares with the names that could stand in for it: everything before its last
    `/` or `#` (the project of `p/s`, the server of `/api/mcp#tool`)."""
    cut = max(name.rfind("/"), name.rfind("#"))
    return name[:cut] if cut > 0 else ""


def compare(res, kind: str, prod: list[str], jc: set[str] | None, renames: list[Rename], answers,
            call: str, why_missing: str = "", note: str = "", kept: dict[str, str] | None = None) -> list[str]:
    """One row per prod name of `kind`; answers the extra jc names (report lines).

    `kept` names, per prod name, the component that stays beside jc and serves it (GOAL-jc-dev
    "Scope of jc on dev"): such a name missing on jc is a skip "kept component: ...", listed is a pass.

    `jc` is None when jc's listing could not be read: every row then fails with that reason.
    `answers(old)` -> (ok, detail): whether the old address of a renamed name answers on jc."""
    by_old = {r.old: r for r in renames if r.kind == kind and not r.extra}
    prod_set = set(prod)
    if jc is None:
        for name in sorted(prod_set):
            res.check(f"{kind}", name, call, "listed on jc", f"jc's listing unreadable: {why_missing}", False, note)
        return []
    renamed_to = {r.new for r in by_old.values() if r.old in prod_set}
    extra = sorted(jc - prod_set - renamed_to)
    for name in sorted(prod_set):
        reason = note
        if name in jc:
            res.check(kind, name, call, "listed on jc", "listed", True, reason)
            continue
        r = by_old.get(name)
        if r is None and name in (kept or {}):
            res.skip(kind, name, call, "listed on jc", f"kept component: {kept[name]}")
            continue
        if r is None:
            near = [x for x in extra if scope(x) == scope(name)][:5]
            why = "not listed on jc, and RENAMES.md lists no rename of it"
            if near:
                why += f"; jc lists instead: {', '.join(near)}"
            res.check(kind, name, call, "listed on jc", why, False, reason)
            continue
        ok, detail = answers(name)
        new_ok = r.new in jc
        res.check(kind, name, call, f"renamed to {r.new} (listed) and the old address answers",
                  f"new {'listed' if new_ok else 'NOT listed'}; old address: {detail}", new_ok and ok,
                  f"RENAMES.md: {r.why}; old clients: {r.keeps}")
    return extra


def stale_renames(res, renames: list[Rename], prod_by_kind: dict) -> None:
    """A listed rename whose old name production does not hold names nothing, and an extra whose
    name production holds is no extra: a row each, so the list stays true."""
    for r in renames:
        if r.extra:
            if r.kind in prod_by_kind and r.new in prod_by_kind[r.kind]:
                res.check("renames.stale", f"{r.kind} (extra) {r.new}", "RENAMES.md", "a name prod never held",
                          "production holds this name", False)
            continue
        if r.kind in prod_by_kind and r.old not in prod_by_kind[r.kind]:
            res.check("renames.stale", f"{r.kind} {r.old}", "RENAMES.md", "the old name is a prod name",
                      "production holds no such name", False)


def skip_kind(res, kind: str, target: str, call: str, reason: str) -> None:
    res.add(kind, target, call, "listed on jc", "", SKIP, reason)

