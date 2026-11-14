#!/usr/bin/env python3
"""Records Bento 1.22.0's output for the ekiben-text parity cases (jq, jmespath, grok, msgpack, parse_log).

crates/ekiben-text/tests/parity/cases.json lists cases {name, processor, input} (`input_hex` for bytes that are not UTF-8); each input runs through
.bento/bento (scripts/reference-bento.sh) as one message with only the metadata `source: eea`, then the
processor, then a mapping writing each message as its content (hex for MessagePack output) or
"ERROR: " + error(). `expected` is that stdout. A case named "... then to_json" reads MessagePack back with
`msgpack: to_json`, since Go writes map keys in random order. tests/parity.rs replays the cases without Bento.
"""
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BENTO = ROOT / ".bento" / "bento"
CASES = ROOT / "crates/ekiben-text/tests/parity/cases.json"


def run(case):
    procs = [json.dumps(case["processor"])]
    if case["name"].endswith("then to_json"):
        procs.append(json.dumps({"switch": [{"check": "!errored()", "processors": [{"msgpack": {"operator": "to_json"}}]}]}))
    hexed = case["processor"].get("msgpack", {}).get("operator") == "from_json" and not case["name"].endswith("then to_json")
    body = 'content().encode("hex")' if hexed else "content().string()"
    with tempfile.TemporaryDirectory() as d:
        inp = Path(d) / "in"
        inp.write_bytes(bytes.fromhex(case["input_hex"]) if "input_hex" in case else case["input"].encode())
        conf = f"""
input:
  file: {{ paths: [ {json.dumps(str(inp))} ], scanner: {{ to_the_end: {{}} }} }}
pipeline:
  processors:
    - mapping: |
        meta = deleted()
        meta source = "eea"
        root = content()
""" + "".join(f"    - {p}\n" for p in procs) + f"""    - mapping: 'root = if errored() {{ "ERROR: " + error() }} else {{ {body} }}'
output:
  stdout: {{ codec: lines }}
logger: {{ level: none, format: logfmt }}
"""
        cf = Path(d) / "c.yaml"
        cf.write_text(conf)
        r = subprocess.run([str(BENTO), "-c", str(cf)], capture_output=True, timeout=60, stdin=subprocess.DEVNULL)
        if r.returncode != 0:
            # Rerun with error logs to read why the processor was refused.
            cf.write_text(conf.replace("level: none", "level: error"))
            r = subprocess.run([str(BENTO), "-c", str(cf)], capture_output=True, timeout=60, stdin=subprocess.DEVNULL)
            out = (r.stderr + r.stdout).decode(errors="replace")
            m = re.search(r'Service closing due to: failed to init processor [^:]*: (.*?)" @service', out)
            return {"config_error": m.group(1).encode().decode("unicode_escape") if m else out.strip().splitlines()[-1]}
    return {"text": r.stdout.decode()}


def main():
    cases = json.loads(CASES.read_text())
    only = set(sys.argv[1:])
    for c in cases:
        if not only or c["name"] in only:
            c["expected"] = run(c)
    CASES.write_text(json.dumps(cases, indent=1, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
