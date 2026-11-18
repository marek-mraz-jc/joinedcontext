#!/usr/bin/env python3
"""A dated cargo-deny ignore expires (T-7232): every `[advisories] ignore` entry of every tracked
deny.toml whose reason says `remove by YYYY-MM-DD` turns this check red once that day has passed
(UTC), so a temporary ignore cannot outlive its date unnoticed. An entry without such a date is not
judged here; an unreadable date is red.

`--today YYYY-MM-DD` judges as of that day (the red proof: a day after the date). `--selftest`
judges a synthetic repository with one case per outcome and fails unless each is judged as expected.
"""

import datetime
import pathlib
import re
import subprocess
import sys
import tempfile
import tomllib

REMOVE_BY = re.compile(r"remove by (\S+?)[,;.)]?(?:\s|$)")


def deny_files(root):
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "*deny.toml", "deny.toml"],
        check=True, capture_output=True, text=True,
    ).stdout
    return sorted({root / line for line in out.splitlines() if line})


def problems(root, files, today):
    """One message per dated ignore that is past its date or whose date does not parse."""
    found = []
    for path in files:
        ignores = tomllib.loads(path.read_text()).get("advisories", {}).get("ignore", [])
        for entry in ignores:
            if not isinstance(entry, dict):
                continue
            for raw in REMOVE_BY.findall(entry.get("reason", "")):
                name = f"{path.relative_to(root)}: {entry.get('id', '?')}"
                try:
                    due = datetime.date.fromisoformat(raw)
                except ValueError:
                    found.append(f"{name}: 'remove by {raw}' is not a YYYY-MM-DD date")
                    continue
                if today > due:
                    found.append(f"{name}: ignore expired on {due} (today {today}); "
                                 "upgrade or migrate off the crate and drop the entry")
    return found


def selftest():
    cases = {
        "past": ("remove by 2027-02-18 or when T-1 lands", "2027-02-19", True),
        "same-day": ("remove by 2027-02-18, or earlier", "2027-02-18", False),
        "future": ("remove by 2027-02-18.", "2026-11-18", False),
        "undated": ("drop when the crate releases", "2030-01-01", False),
        "bad-date": ("remove by 2027-02-30 or when T-1 lands", "2026-11-18", True),
    }
    failed = []
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        for name, (reason, today, red) in cases.items():
            path = root / f"{name}.toml"
            path.write_text(f'[advisories]\nignore = ["RUSTSEC-0000-0000", '
                            f'{{ id = "RUSTSEC-0000-0001", reason = "{reason}" }}]\n')
            got = problems(root, [path], datetime.date.fromisoformat(today))
            if bool(got) != red:
                failed.append(f"{name}: expected {'red' if red else 'green'}, got {got}")
    if failed:
        print("deny-ignore-dates selftest FAILED:\n  " + "\n  ".join(failed))
        return 1
    print(f"deny-ignore-dates selftest: {len(cases)} cases judged as expected")
    return 0


def main(argv):
    if argv == ["--selftest"]:
        return selftest()
    today = datetime.datetime.now(datetime.timezone.utc).date()
    if len(argv) == 2 and argv[0] == "--today":
        today = datetime.date.fromisoformat(argv[1])
    elif argv:
        print("usage: .github/scripts/deny-ignore-dates.py [--selftest | --today YYYY-MM-DD]",
              file=sys.stderr)
        return 2
    root = pathlib.Path(__file__).resolve().parents[2]
    files = deny_files(root)
    found = problems(root, files, today)
    for line in found:
        print(f"::error::{line}")
    if not found:
        print(f"deny-ignore-dates: {len(files)} deny.toml files, no dated ignore past {today}")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
