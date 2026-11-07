#!/usr/bin/env python3
"""Fill the ICS pro forma of ETSI TS 104 187 V1.1.1 (annex A.5) for vanilla Antares from the
conformance ledger (docs/spec), and write docs/analysis/ics-ts104187.md (T-3435); with --full, the
same statement for the `full` build's NGSI-LD root (T-3524).

    python3 dev/ics-fill.py [ETSI_SPECS_DIR] > docs/analysis/ics-ts104187.md
    python3 dev/ics-fill.py --full [ETSI_SPECS_DIR] > docs/analysis/ics-ts104187-full.md

The output names the ledger's last commit, not HEAD, so a rerun on any later commit that leaves
the ledger alone is byte for byte the committed file.

TS 104 187 A.0 lets anyone reproduce and publish the completed pro forma. Its "Clauses" column
cites GS CIM 009 (A.3), which is the ledger's numbering. An item's support is read from the ledger
status of every clause it cites, a heading clause taking the status of the clauses under it:
`Y` when all of them are implemented, `Y (partial)` when some are, `N` when none is. The new home
of each cited clause comes from dev/clause-map.py.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FULL = "--full" in sys.argv[1:]
ARGS = [a for a in sys.argv[1:] if a != "--full"]
SPECS = Path(ARGS[0] if ARGS else "/workspace/etsi-specs")
ICS = SPECS / "ts-104187-ngsi-ld-ics" / "A"
HEADER = "Item Feature Subfeature Clauses Mnemonic Support"
CLAUSE = re.compile(r"^[0-9A-H][0-9.]*(\s*;\s*[0-9A-H][0-9.]*)*$")
JOINED = re.compile(r"^((?:[0-9][0-9.]*\s*;\s*)*[0-9][0-9.]*)\s+(\S+)$")
# The pro forma's own slips, read against the ledger's titles: the clause that defines the item.
CORRECTIONS = {("QueTemEvoEntFor", "5.15.4"): "5.14.5"}
# Items whose cited clauses are informative only, so the ledger carries no status to read: the
# answer and the evidence it rests on, by hand.
HAND = {
    "CENTRALIZED": ("Y", "one broker holds and serves the context itself: the default deployment "
                    "(ARCHITECTURE.md, `antares-store`)"),
    "DISTRIBUTED": ("Y", "registrations forward requests to Context Sources: "
                    "[4.3.6.2](../spec/4/4.3.6.2.md), [4.3.6.3](../spec/4/4.3.6.3.md) implemented, "
                    "`crates/antares-api/src/federation.rs`"),
    "FEDERATED": ("Y", "a federation is brokers registered as each other's Context Sources, the "
                  "same forwarding as DISTRIBUTED: [4.3.6.4](../spec/4/4.3.6.4.md) limits its "
                  "cascade, implemented"),
}


def split(path: Path):
    text = path.read_text(encoding="utf-8")
    if text.startswith("---\n"):
        head, _, body = text[4:].partition("\n---\n")
        return head, body
    return "", text


def field(head: str, name: str) -> str:
    match = re.search(rf"^{name}:\s?(.*(?:\n\s+.*)*)", head, re.M)
    return re.sub(r"\s+", " ", match.group(1)).strip().strip("'\"") if match else ""


def ledger():
    clauses = {}
    for path in (ROOT / "docs" / "spec").rglob("*.md"):
        head, _ = split(path)
        number = field(head, "clause")
        if number:
            clauses[number] = {
                "title": field(head, "title"),
                "status": field(head, "status"),
                "path": path.relative_to(ROOT).as_posix(),
            }
    return clauses


def subtree_status(number: str, clauses) -> str:
    """implemented / partial / not-implemented over the clause and every clause under it that is
    not informative; `informative` when nothing under it carries a status of its own."""
    own = [
        c["status"]
        for key, c in clauses.items()
        if (key == number or key.startswith(number + ".")) and c["status"] != "informative"
    ]
    if not own:
        return "informative"
    if all(s == "implemented" for s in own):
        return "implemented"
    if any(s in ("implemented", "partial") for s in own):
        return "partial"
    return "not-implemented"


def items():
    def order(path: Path):
        return [int(part) for part in path.stem[2:].split(".") if part.isdigit()]

    for path in sorted(ICS.glob("A.5.*.md"), key=order):
        _, body = split(path)
        lines = [line.strip() for line in body.splitlines() if line.strip()]
        lines = [line.replace(HEADER, "").strip() for line in lines]
        if "Support" not in lines:
            continue
        table = lines[0]
        buffer = []
        for line in lines[lines.index("Support") + 1 :]:
            if line != "Y/N":
                if line:
                    buffer.append(line)
                continue
            mnemonic, clauses = buffer.pop() if buffer else "", ""
            joined = JOINED.match(mnemonic)
            if joined:
                clauses, mnemonic = joined.group(1), joined.group(2)
            elif buffer and CLAUSE.match(buffer[-1]):
                clauses = buffer.pop()
            yield table, " ".join(buffer), clauses, mnemonic
            buffer = []


def new_homes():
    rows = subprocess.run(
        [sys.executable, str(ROOT / "dev" / "clause-map.py"), str(SPECS)],
        capture_output=True, text=True, check=True,
    ).stdout.splitlines()[1:]
    homes = {}
    for row in rows:
        cells = row.split("\t")
        if cells[0] and cells[3]:
            homes[cells[0]] = f"{cells[3]} {cells[4]}"
    return homes


def main() -> None:
    clauses, homes = ledger(), new_homes()
    commit = subprocess.run(
        ["git", "-C", str(ROOT), "log", "-1", "--format=%h", "--", "docs/spec"],
        capture_output=True, text=True,
    ).stdout.strip()
    tables, counts = {}, {"Y": 0, "Y (partial)": 0, "N": 0}
    for table, feature, cited, mnemonic in items():
        refs = []
        for number in [c.strip() for c in cited.split(";") if c.strip()]:
            refs.append(CORRECTIONS.get((mnemonic, number), number))
        statuses = [subtree_status(n, clauses) for n in refs if n in clauses]
        statuses = [s for s in statuses if s != "informative"] or statuses
        if statuses and all(s == "implemented" for s in statuses):
            support = "Y"
        elif any(s in ("implemented", "partial") for s in statuses):
            support = "Y (partial)"
        else:
            support = "N"
        evidence = ""
        if mnemonic in HAND and all(subtree_status(n, clauses) == "informative" for n in refs):
            support, evidence = HAND[mnemonic][0], "by hand: " + HAND[mnemonic][1]
        counts[support] += 1
        evidence = evidence or "; ".join(
            f"[{n}](../{clauses[n]['path'][5:]}) {subtree_status(n, clauses)}"
            if n in clauses
            else f"{n} not in the ledger"
            for n in refs
        )
        moved = "; ".join(homes.get(n, "unmatched") for n in refs) or "n/a"
        tables.setdefault(table, []).append((feature, cited, mnemonic, support, evidence, moved))

    build = "the `full` build" if FULL else "vanilla Antares"
    out = [
        "---",
        f"title: ICS of {build} against ETSI TS 104 187 V1.1.1",
        f"task: {'T-3524' if FULL else 'T-3435'}",
        "---",
        "",
        f"# ICS of {build} (ETSI TS 104 187 V1.1.1, annex A)",
        "",
        f"Generated by `python3 dev/ics-fill.py{' --full' if FULL else ''}` from the conformance "
        f"ledger `docs/spec` at `{commit}`; rerun it after a ledger change, edit the ledger, not this "
        "file. ETSI grants the right to reproduce and publish the completed pro forma (TS 104 187 A.0).",
        "",
        *([
            "The support column is vanilla's ([ics-ts104187.md](ics-ts104187.md)), and a test holds "
            "it: E3, `image-jc.yml` job `etsi`, runs every serial suite on the `full,jc-policy` image's NGSI-LD "
            "root and checks it TP by TP against `dev/etsi-baseline.json`, the TPs vanilla passes. The "
            "surfaces the build adds are no NGSI-LD API root of TS 104 187; their conformance "
            "statements, each with its suite, version and job, are "
            "[../src/surfaces.md](../src/surfaces.md) section 5. Through a view (E2, job "
            "`etsi-full-all`) a TP answers as at the root unless the view owes the refusal; the known "
            "exceptions are in `dev/etsi-view-known.json`, each with its task.",
            "",
        ] if FULL else []),
        "## 1. Identification (A.2, A.3)",
        "",
        "| | |",
        "|---|---|",
        "| Implementation under test | Antares NGSI-LD Context Broker, "
        + ("cargo features `full` (views, mcp, odata, ogc, ws), its NGSI-LD root `/ngsi-ld/v1` |"
           if FULL else "default cargo features (vanilla) |"),
        "| Repository | github.com/marek-mraz-jc/Antares-NGSI-LD-Context-Broker, branch `dev` |",
        f"| Commit | `{commit}` |",
        "| Reference specification of the Clauses column | ETSI GS CIM 009, as A.3 states; read here in "
        "the ledger's V1.9.1 numbering, which the cited numbers match by title (one slip corrected, "
        "listed in section 4) |",
        "",
        "## 2. Global statement of conformance (A.4)",
        "",
        "Are all mandatory capabilities implemented? "
        + ("**Yes.**" if counts["N"] == 0 and counts["Y (partial)"] == 0 else "**No.**")
        + f" {counts['N']} items are not supported and {counts['Y (partial)']} only in part:",
        "",
        *[f"- {table} / {mnemonic}: {support}, {evidence}" for table, rows in tables.items()
          for (_, _, mnemonic, support, evidence, _) in rows if support != "Y"],
        "",
        "The pro forma's tables carry no status column (m/o), so which items TS 104 187 makes "
        "mandatory is not stated per item; every item short of Y is listed above rather than "
        "guessed. The statement is against GS CIM 009 as the pro forma asks (A.3), as the ledger "
        "audits it; what the TC DATA family changed since is in ts104175-delta.md, and a clause "
        "found non-compliant there is not reflected here until the ledger records it.",
        "",
        f"Totals: {counts['Y']} Y, {counts['Y (partial)']} Y (partial), {counts['N']} N, "
        f"{sum(counts.values())} items.",
        "",
        "## 3. Tables (A.5)",
        "",
        "Support: `Y` every cited ledger clause (with the clauses under it) is implemented, "
        "`Y (partial)` some are, `N` none is. Evidence: the ledger clause and its derived status; "
        "the ledger file holds the code and test anchors. New home: where the cited clause's text "
        "is in TS 104 175 / 104 176 now (`dev/clause-map.py`).",
        "",
    ]
    for table, rows in tables.items():
        out += [f"### {table}", "", "| Item | Feature | Clauses | Mnemonic | Support | Evidence (ledger) | New home |",
                "|---|---|---|---|---|---|---|"]
        for number, (feature, cited, mnemonic, support, evidence, moved) in enumerate(rows, 1):
            out.append(f"| {number} | {feature} | {cited} | {mnemonic} | {support} | {evidence} | {moved} |")
        out.append("")
    out += [
        "## 4. Corrections to the pro forma",
        "",
        "- A.5.2.26 (`QueTemEvoEntFor`) cites 5.15.4, which is Context Source Identity Information in "
        "the ledger; the operation it names, Create EntityMap for Query Temporal Evolution of "
        "Entities, is 5.14.5. The support is read from 5.14.5.",
        "",
        "## Related",
        "",
        "- [ts104175-delta.md](ts104175-delta.md): the clause map and the normative delta.",
        "- [../spec/README.md](../spec/README.md): the ledger this statement is read from.",
        *(["- [ics-ts104187.md](ics-ts104187.md): the vanilla statement.",
           "- [../src/surfaces.md](../src/surfaces.md): the surfaces of the build and their "
           "conformance statements."] if FULL else []),
        "",
    ]
    print("\n".join(out))


if __name__ == "__main__":
    main()
