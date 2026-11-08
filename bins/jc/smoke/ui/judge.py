"""The UI judge's input and output for a `ui` run (JUDGE.md).

    python3 -I judge.py pack  --out <out> [--batch 40]      shots.json -> judge/batch-NN.json
    python3 -I judge.py merge --out <out> <answer.json>...  -> judge.json (last answer per page wins)
    python3 -I judge.py fold  --out <out>                   judge.json -> results.json + REPORT.md
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))

from common import Results  # noqa: E402


def pack(out: Path, batch: int) -> list[Path]:
    rows = json.loads((out / "shots.json").read_text())
    d = out / "judge"
    d.mkdir(exist_ok=True)
    files = []
    for i in range(0, len(rows), batch):
        f = d / f"batch-{i // batch:02d}.json"
        f.write_text(json.dumps([{**r, "file": str((out / r["file"]).resolve())} for r in rows[i:i + batch]], indent=1))
        files.append(f)
    return files


def merge(out: Path, answers: list[Path]) -> Path:
    by = {}
    for f in answers:
        text = f.read_text().strip()
        # A subagent may wrap its JSON in a fence.
        if text.startswith("```"):
            text = text.split("\n", 1)[1].rsplit("```", 1)[0]
        for row in json.loads(text):
            by[(row["page"], int(row["width"]))] = row
    f = out / "judge.json"
    f.write_text(json.dumps(sorted(by.values(), key=lambda r: (r["page"], r["width"])), indent=1, ensure_ascii=False))
    return f


def fold(out: Path) -> dict:
    import suite  # the same rule the walk applies

    doc = json.loads((out / "results.json").read_text())
    res = Results(doc["suite"], doc["base"])
    res.rows = [r for r in doc["checks"] if r["target"] != "judge"]
    suite.fold_judge(res, json.loads((out / "shots.json").read_text()), str(out / "judge.json"))
    meta = {k: v for k, v in doc.items() if k not in ("suite", "base", "seconds", "totals", "checks")}
    res.started -= doc.get("seconds", 0)
    res.write(out, meta)
    return res.totals()


def main(argv=None) -> int:
    p = argparse.ArgumentParser()
    p.add_argument("cmd", choices=["pack", "merge", "fold"])
    p.add_argument("--out", required=True, type=Path)
    p.add_argument("--batch", type=int, default=40)
    p.add_argument("answers", nargs="*", type=Path)
    a = p.parse_args(argv)
    if a.cmd == "pack":
        print("\n".join(map(str, pack(a.out, a.batch))))
    elif a.cmd == "merge":
        print(merge(a.out, a.answers))
    else:
        print(json.dumps(fold(a.out)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
