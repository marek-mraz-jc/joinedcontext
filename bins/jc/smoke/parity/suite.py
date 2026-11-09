"""Suite parity: the new pages against the old pages captured from dev.

  run parity --base-url http://127.0.0.1:<port> --person-file <file '<user> <password>'> --old-dir <evidence dir> --out <dir> [--pairs a,b]

The old side is never touched here: --old-dir holds its captures (`parity.py capture --side old`).
Each feature of an old page becomes a check that the new page has it; a page or feature the old side
lacks is a skip with the reason."""

import argparse
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import pages as P  # noqa: E402
import parity  # noqa: E402


def main(argv):
    ap = argparse.ArgumentParser(prog="run parity")
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--person-file", required=True)
    ap.add_argument("--old-dir", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--pairs")
    ap.add_argument("--admin-token-file")  # accepted for the common runner; parity signs in as a person
    ap.add_argument("--person-token-file")
    a = ap.parse_args(argv)
    out, old = Path(a.out), Path(a.old_dir)
    pairs = a.pairs.split(",") if a.pairs else list(P.PAIRS)
    for pair in pairs:
        src = old / pair / "raw" / "old"
        if src.is_dir() and src.resolve() != (out / pair / "raw" / "old").resolve():
            shutil.copytree(src, out / pair / "raw" / "old", dirs_exist_ok=True)
        if (old / pair / "vars-old.json").exists() and old.resolve() != out.resolve():
            shutil.copy(old / pair / "vars-old.json", out / pair / "vars-old.json")
    parity.main(["capture", "--side", "new", "--base", a.base_url, "--person-file", a.person_file,
                 "--pairs", ",".join(pairs), "--out", str(out)])
    parity.main(["compose", "--pairs", ",".join(pairs), "--out", str(out)])
    n = parity.write_report(out, parity.report(out, pairs))
    print(f"parity: {n}")
    return 1 if n["fail"] else 0
