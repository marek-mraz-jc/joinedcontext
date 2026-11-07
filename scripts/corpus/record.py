#!/usr/bin/env python3
"""Fill stdout, stderr and code of every case in crates/ekiben-server/tests/parity/corpus_regressions.json from
`.bento/bento blobl` (Bento 1.22.0), so `cargo test -p ekiben-server --test corpus_regressions` checks ekiben
against Bento's recorded output without the Bento binary. Usage: python3 scripts/corpus/record.py"""
import json
import os
import subprocess

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CASES = os.path.join(ROOT, "crates", "ekiben-server", "tests", "parity", "corpus_regressions.json")

cases = json.load(open(CASES))
for c in cases:
    p = subprocess.run([os.path.join(ROOT, ".bento", "bento"), "blobl", c["mapping"]], capture_output=True,
                       input="".join(l + "\n" for l in c["input"]).encode(), timeout=20)
    c["stdout"], c["stderr"], c["code"] = p.stdout.decode(), p.stderr.decode(), p.returncode
with open(CASES, "w") as f:
    json.dump(cases, f, indent=2, ensure_ascii=False)
    f.write("\n")
print(f"recorded {len(cases)} cases")
