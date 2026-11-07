#!/usr/bin/env python3
"""Records Bento 1.22.0's output for the parquet_decode and protobuf parity cases.

Each crates/<crate>/tests/parity/cases.json lists cases {name, processor, input}; this script runs .bento/bento
(scripts/reference-bento.sh) on every input and writes back `expected`: the output text when it is short, else its
SHA-256 and length, or the error text of a failed message. The crate's parity test replays the cases without Bento.
"""
import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BENTO = ROOT / ".bento" / "bento"
INLINE_MAX = 4096


def run(case, base):
    proc = json.dumps(case["processor"])
    pb = case["processor"].get("protobuf", {})
    if pb.get("operator") == "from_json":
        # proto.Marshal of a dynamic message writes fields in Go map order, different on every run: the case
        # records the message converted back by to_json, which is deterministic.
        back = {"operator": "to_json", "message": pb["message"], "import_paths": pb["import_paths"]}
        proc += "\n    - " + json.dumps({"switch": [{"check": "!errored()", "processors": [{"protobuf": back}]}]})
    conf = f"""
input:
  file: {{ paths: [ {json.dumps(str(base / case["input"]))} ], scanner: {{ to_the_end: {{}} }} }}
pipeline:
  processors:
    - {proc}
    - mapping: 'root = if errored() {{ "ERROR: " + error() }} else {{ content() }}'
output:
  stdout: {{ codec: lines }}
logger: {{ level: none }}
"""
    with tempfile.NamedTemporaryFile("w", suffix=".yaml") as f:
        f.write(conf)
        f.flush()
        r = subprocess.run([str(BENTO), "-c", f.name], cwd=base, capture_output=True, timeout=60, stdin=subprocess.DEVNULL)
    if r.returncode != 0:
        return {"config_error": r.stderr.decode(errors="replace").strip().splitlines()[-1]}
    out = r.stdout
    if out.startswith(b"ERROR: "):
        return {"error": out[7:].rstrip(b"\n").decode()}
    if len(out) <= INLINE_MAX:
        try:
            return {"text": out.decode()}
        except UnicodeDecodeError:
            pass
    return {"sha256": hashlib.sha256(out).hexdigest(), "len": len(out), "lines": out.count(b"\n")}


def main():
    for cases_file in sys.argv[1:] or sorted(ROOT.glob("crates/*/tests/parity/cases.json")):
        cases_file = Path(cases_file).resolve()
        cases = json.loads(cases_file.read_text())
        for case in cases:
            case["expected"] = run(case, cases_file.parent)
            print(case["name"], list(case["expected"])[0])
        cases_file.write_text(json.dumps(cases, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
