#!/usr/bin/env python3
"""Regenerate crates/ekiben-bloblang/tests/parity/examples.json from Bento's own docs.

Every ```coffee block of website/docs/guides/bloblang/{methods,functions}.md at tag v1.22.0
holding `# In:` lines becomes one mapping with its inputs. Each input runs through the
reference binary, one process per case, exactly as a person would:

    printf '%s\n' "$input" | .bento/bento blobl "$mapping"

stdout and stderr are recorded byte for byte beside the input, so the parity test in
ekiben-bloblang checks them without the Bento binary.

Usage (from the repo root):
    sh scripts/reference-bento.sh
    git clone --depth 1 -b v1.22.0 https://github.com/warpstreamlabs/bento <dir>
    python3 scripts/parity-examples.py <dir>
"""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BENTO = ROOT / ".bento" / "bento"
OUT = ROOT / "crates" / "ekiben-bloblang" / "tests" / "parity" / "examples.json"


def blocks(path: Path):
    """Yield (heading, mapping, [inputs]) per coffee block with `# In:` cases."""
    heading = ""
    in_block = False
    lines: list[str] = []
    for line in path.read_text().splitlines():
        if not in_block:
            m = re.match(r"^### `([^`]+)`", line)
            if m:
                heading = m.group(1)
            if line.strip() == "```coffee":
                in_block, lines = True, []
            continue
        if line.strip() == "```":
            in_block = False
            inputs = [l[len("# In:"):].strip() for l in lines if l.startswith("# In:")]
            outs = [l[len("# Out:"):].strip() for l in lines if l.startswith("# Out:")]
            if inputs:
                first = next(i for i, l in enumerate(lines) if l.startswith("# In:"))
                mapping = "\n".join(lines[:first]).strip()
                yield heading, mapping, list(zip(inputs, outs))
            continue
        lines.append(line)


FIXTURES = OUT.parent
# Bento's own example tests (internal/impl/io/bloblang_examples_test.go) point these at
# temp files with this content; the parity test sets them to the same committed files.
ENV = {
    "BENTO_TEST_BLOBLANG_FILE": str(FIXTURES / "file.json"),
    "BENTO_TEST_BLOBLANG_SCHEMA_FILE": str(FIXTURES / "schema.json"),
}


def run(mapping: str, value: str) -> dict:
    p = subprocess.run(
        [str(BENTO), "blobl", mapping],
        env={"PATH": "/usr/bin:/bin", "TZ": "UTC", **ENV},
        input=(value + "\n").encode(),
        capture_output=True,
        timeout=30,
    )
    return {"stdout": p.stdout.decode(), "stderr": p.stderr.decode(), "code": p.returncode}


def main() -> int:
    docs = Path(sys.argv[1]) / "website" / "docs" / "guides" / "bloblang"
    cases = []
    for page in ("methods.md", "functions.md"):
        for heading, mapping, inputs in blocks(docs / page):
            for value, docs_out in inputs:
                cases.append({
                    "page": page,
                    "name": heading,
                    "mapping": mapping,
                    "input": value,
                    "docs_out": docs_out,
                    "bento": run(mapping, value),
                })
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(cases, indent=1, ensure_ascii=False) + "\n")
    print(f"{len(cases)} cases written to {OUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
