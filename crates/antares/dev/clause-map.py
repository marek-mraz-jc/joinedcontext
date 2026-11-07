#!/usr/bin/env python3
"""Map every clause of the GS CIM 009 v1.9.1 ledger (docs/spec) to the clause of the TC DATA
family that carries its text now (TS 104 175 / 104 176 / 104 243 / 104 179, /workspace/etsi-specs),
by heading and text, never by number (T-3435).

    python3 dev/clause-map.py [ETSI_SPECS_DIR] > table.tsv
    python3 dev/clause-map.py --markdown [ETSI_SPECS_DIR]   # the appendix of docs/analysis/ts104175-delta.md

One row per ledger clause: old clause, old title, status, new document, new clause, new title,
score, `kind`, and the normative sentences (shall/should/may/must) the old clause has and the
new one lacks, and the other way round (`rules_removed`, `rules_added`, counts only), and `kind`:
  moved     same title, the text kept (score >= 1.4)
  edited    the title kept with part of the text, or at least 0.6 of the text kept
  weak      0.45 to 0.6 of the text found under another title: likely, but check by hand
  title     the clause's text is not found; only a title that is not generic matches (hand check)
  heading   the clause holds no text of its own (a heading over its subclauses), nothing matches
  unmatched none of the above
Then one row per new clause no ledger clause landed on (`new-only`).

Score: the share of the old clause's word trigrams found in the candidate (containment, so a clause
split across two documents still finds the half that holds most of it), plus 0.5 for an equal
normalised title unless the title is generic ("Introduction", "General", ...). A clause body under
12 words is matched on its title alone.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MARKDOWN = "--markdown" in sys.argv
ARGS = [a for a in sys.argv[1:] if a != "--markdown"]
SPECS = Path(ARGS[0] if ARGS else "/workspace/etsi-specs")
DOCS = {
    "ts-104175-ngsi-ld-core-api": "TS 104 175",
    "ts-104176-ngsi-ld-http-binding": "TS 104 176",
    "ts-104243-ngsi-ld-mqtt-notification-binding": "TS 104 243",
    "ts-104179-ngsi-ld-provenance-integrity": "TS 104 179",
}
MATCH = 0.45  # below this a clause is unmatched; measured on the run recorded in the analysis
# Titles that say nothing about the clause: an equal one is no evidence of the same clause.
GENERIC = {
    "introduction", "general", "foreword", "overview", "description", "http request",
    "http response", "examples", "example", "definitions", "abbreviations", "scope", "references",
}


def parse(path: Path):
    text = path.read_text(encoding="utf-8")
    front, body = {}, text
    if text.startswith("---\n"):
        head, _, body = text[4:].partition("\n---\n")
        for line in head.splitlines():
            key, sep, value = line.partition(":")
            if sep and not line.startswith(" "):
                front[key.strip()] = value.strip().strip("'\"")
    return front, body


def words(text: str):
    return re.findall(r"[a-z0-9@]+", text.lower())


def norm_title(title: str) -> str:
    return " ".join(words(title))


NORMATIVE = re.compile(r"\b(shall|should|may|must|can not|cannot)\b", re.I)


def rules(body: str):
    """The clause's normative sentences (shall, should, may), each as its normalised words, so a
    rule that only moved or changed its line breaks compares equal."""
    flat = re.sub(r"\s+", " ", body)
    sentences = re.split(r"(?<=[.;:])\s+(?=[A-Z\u2022])|\u2022", flat)
    return {" ".join(words(s)) for s in sentences if NORMATIVE.search(s) and len(words(s)) > 3}


def trigrams(tokens):
    return {tuple(tokens[i : i + 3]) for i in range(len(tokens) - 2)}


def load(directory: Path, label: str):
    clauses = []
    for path in sorted(directory.rglob("*.md")):
        if path.name == "README.md":
            continue
        front, body = parse(path)
        if "clause" not in front:
            continue
        tokens = words(body)
        clauses.append(
            {
                "rules": rules(body),
                "doc": label,
                "clause": front["clause"],
                "title": front.get("title", ""),
                "status": front.get("status", ""),
                "ntitle": norm_title(front.get("title", "")),
                "grams": trigrams(tokens),
                "short": len(tokens) < 12,
            }
        )
    return clauses


def emit(cells) -> None:
    if not MARKDOWN:
        print("\t".join(cells))
        return
    old, title, status, doc, new, new_title, score, kind, removed, added = cells
    if old:
        chapter = old.split(".")[0]
        old = f"[{old}](../spec/{chapter}/{old}.md)"
    print(f"| {old} | {title} | {status} | {doc} {new} | {new_title} | {kind} | {score} | {removed} | {added} |")


def main() -> None:
    old = load(ROOT / "docs" / "spec", "GS CIM 009")
    new = [c for folder, label in DOCS.items() for c in load(SPECS / folder, label)]
    by_title = {}
    for candidate in new:
        by_title.setdefault(candidate["ntitle"], []).append(candidate)
    # Inverted index from trigram to the new clauses holding it, so a ledger clause is compared
    # only with candidates that share text with it.
    index = {}
    for position, candidate in enumerate(new):
        for gram in candidate["grams"]:
            index.setdefault(gram, []).append(position)

    hit = set()
    if MARKDOWN:
        print("| CIM 009 | Title | Ledger status | New clause | New title | Kind | Score | Rules removed | Rules added |")
        print("|---|---|---|---|---|---|---|---|---|")
    else:
        print("old_clause\told_title\tstatus\tnew_doc\tnew_clause\tnew_title\tscore\tkind\trules_removed\trules_added")
    for clause in old:
        shared = {}
        for gram in clause["grams"]:
            for position in index.get(gram, ()):
                shared[position] = shared.get(position, 0) + 1
        best, best_score = None, 0.0
        size = max(len(clause["grams"]), 1)
        candidates = set(shared) | {new.index(c) for c in by_title.get(clause["ntitle"], [])}
        for position in candidates:
            candidate = new[position]
            text = 0.0 if clause["short"] else shared.get(position, 0) / size
            same = clause["ntitle"] and clause["ntitle"] == candidate["ntitle"]
            title = 0.5 if same and clause["ntitle"] not in GENERIC else 0.0
            score = text + title if not clause["short"] else title * 2
            if score > best_score:
                best, best_score = candidate, score
        if best is None or best_score < MATCH:
            titled = [c for c in by_title.get(clause["ntitle"], []) if clause["ntitle"] not in GENERIC]
            if len(titled) == 1:
                best = titled[0]
                kind = "title"
                row = [best["doc"], best["clause"], best["title"], f"{best_score:.2f}"]
            else:
                kind = "heading" if clause["short"] else "unmatched"
                row = ["", "", "", f"{best_score:.2f}"]
        else:
            hit.add((best["doc"], best["clause"]))
            same_title = clause["ntitle"] == best["ntitle"]
            if same_title and best_score >= 1.4:
                kind = "moved"
            elif same_title or best_score >= 0.6:
                kind = "edited"
            else:
                kind = "weak"
            row = [best["doc"], best["clause"], best["title"], f"{best_score:.2f}"]
        delta = ["", ""]
        if kind in ("moved", "edited", "weak", "title"):
            delta = [str(len(clause["rules"] - best["rules"])), str(len(best["rules"] - clause["rules"]))]
        emit([clause["clause"], clause["title"], clause["status"], *row, kind, *delta])
    for candidate in new:
        if (candidate["doc"], candidate["clause"]) not in hit:
            emit(["", "", "", candidate["doc"], candidate["clause"], candidate["title"], "", "new-only", "", str(len(candidate["rules"]))])


if __name__ == "__main__":
    main()
