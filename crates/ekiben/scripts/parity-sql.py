#!/usr/bin/env python3
"""Records the sql parity cases (tests/parity/sql/<component>/<case>/<n>.yaml) in Bento 1.22.0 and
replays them in ekiben against the same databases.

Each case is a list of configs run in order on a fresh database; a case's record is every step's
exit code and stdout. `record` writes Bento's record to <case>/expected.txt and the comparison to
tests/parity/sql/PARITY.md; `check` compares ekiben with the recorded files only. The lint cases
(tests/parity/sql/lint/*) record `bento lint` and `ekiben lint` verdicts.

Environment: PG_DSN and MY_DSN name the postgres and mysql test databases (postgres and mysql
images, as Bento's integration tests run them); PG_RESET and MY_RESET are the commands that empty
them (for example a `docker exec ... psql` line). sqlite runs on a file in a temporary directory.

    sh scripts/reference-bento.sh
    python3 scripts/parity-sql.py record --ekiben $CARGO_TARGET_DIR/debug/ekiben
"""

import argparse
import os
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
CASES = ROOT / "tests/parity/sql"
BENTO = ROOT / ".bento/bento"
# A case run on a driver whose database is not given is skipped, never failed.
NEEDS = {"postgres": "PG_DSN", "mysql": "MY_DSN"}


def needs(case: pathlib.Path) -> list[str]:
    text = "".join(p.read_text() for p in sorted(case.glob("*.yaml")))
    return [env for drv, env in NEEDS.items() if f"${{{env}}}" in text]


def reset(env: dict) -> None:
    for key in ("PG_RESET", "MY_RESET"):
        if env.get(key):
            subprocess.run(env[key], shell=True, check=True, capture_output=True, timeout=60)


def run(cmd: list[str], env: dict) -> tuple[int, str]:
    try:
        p = subprocess.run(cmd, env=env, capture_output=True, timeout=60, start_new_session=True)
    except subprocess.TimeoutExpired:
        return 124, ""
    return p.returncode, p.stdout.decode("utf-8", "replace")


def record_case(case: pathlib.Path, runner: list[str], env: dict) -> str:
    reset(env)
    with tempfile.TemporaryDirectory() as tmp:
        env = dict(env, LITE_DSN=f"file:{tmp}/lite.db")
        out = []
        for step in sorted(case.glob("*.yaml"), key=lambda p: int(p.stem)):
            code, stdout = run(runner + [str(step)], env)
            out.append(f"## step {step.stem} exit {code}\n{stdout}")
        return "".join(out)


def strip_codes(record: str) -> str:
    """The record with each exit code reduced to ok / failed."""
    out = []
    for line in record.splitlines(keepends=True):
        if line.startswith("## step "):
            head, code = line.rsplit(" exit ", 1)
            line = f"{head} {'ok' if code.strip() == '0' else 'failed'}\n"
        out.append(line)
    return "".join(out)


def lint_verdict(case: pathlib.Path, runner: list[str], env: dict) -> str:
    code, _ = run(runner + [str(case / "1.yaml")], dict(env, LITE_DSN="file:/tmp/lint.db"))
    return "refused" if code else "accepted"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["record", "check"])
    ap.add_argument("--ekiben", required=True)
    args = ap.parse_args()
    env = {k: v for k, v in os.environ.items()}
    # The test databases listen on loopback, which the default outbound rule refuses.
    ekiben_run = [args.ekiben, "run", "--host-env", "--allow-private-ips"]
    bento_run = [str(BENTO), "-c"]
    rows, failed = [], 0
    for case in sorted(p for p in CASES.glob("*/*") if p.is_dir()):
        name = case.relative_to(CASES).as_posix()
        if name.startswith("lint/"):
            b = lint_verdict(case, [str(BENTO), "lint"], env)
            e = lint_verdict(case, [args.ekiben, "lint"], env)
            want = (case / "expected.txt").read_text().strip() if (case / "expected.txt").exists() else None
            if args.mode == "record":
                (case / "expected.txt").write_text(b + "\n")
                want = b
            verdict = "same" if e == want else f"differs (Bento {want}, ekiben {e})"
            if e != want and (case / "deviation.txt").exists():
                verdict = f"deviation (Bento {want}, ekiben {e}: see deviation.txt)"
            rows.append((name, verdict))
            continue
        missing = [n for n in needs(case) if not env.get(n)]
        if missing:
            rows.append((name, f"skipped ({', '.join(missing)} not set)"))
            continue
        if args.mode == "record":
            (case / "expected.txt").write_text(record_case(case, bento_run, env))
        want = (case / "expected.txt").read_text()
        got = record_case(case, ekiben_run, env)
        same = got == want
        # A step both fail on (a driver error Bento retries until the 60 s cap, ekiben gives up on)
        # matches when every step's stdout matches and the same steps fail.
        both_fail = not same and strip_codes(got) == strip_codes(want)
        same = same or both_fail
        if not same:
            failed += 1
            (case / "ekiben.txt").write_text(got)
        elif (case / "ekiben.txt").exists():
            (case / "ekiben.txt").unlink()
        rows.append((name, ("both fail" if both_fail else "identical") if same else "different (see ekiben.txt)"))
    if args.mode == "record":
        lines = ["# sql parity with Bento 1.22.0", "",
                 "Written by `scripts/parity-sql.py record`; each case runs its configs in Bento and in ekiben",
                 "against the same fresh databases (postgres and mysql images, sqlite file).", "",
                 "| Case | Result |", "|---|---|"]
        lines += [f"| {n} | {v} |" for n, v in rows]
        (CASES / "PARITY.md").write_text("\n".join(lines) + "\n")
    for n, v in rows:
        print(f"{v:40} {n}")
    return 1 if failed or any(v.startswith("differs") for _, v in rows) else 0


if __name__ == "__main__":
    sys.exit(main())
