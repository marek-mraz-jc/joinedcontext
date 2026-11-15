"""Under `pipefail`, `producer | grep -q …` answers "no match" at random (T-2657).

`grep -q` exits at its first match; a producer still writing then dies of SIGPIPE, and
`pipefail` reports the pipeline as failed although grep found the line. On a loaded runner a
200-byte `printf` loses that race about once in 600: smoke.sh reported present headers as
missing, skipped its whole Helsinki section (T-2653), and could have passed a ConfigMap that
does hold a resolved credential (T-2660 fixed smoke.sh). A script under `pipefail` matches a
here-string or `< <(command)`, or pipes into a grep that reads all of its input (`>/dev/null`).
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# A single `|` (not `||`) straight into a quiet grep.
PIPED_QUIET_GREP = re.compile(r"(?<!\|)\|(?!\|)\s*grep\s+-[A-Za-z]*q")


def pipefail_scripts() -> list[Path]:
    return [p for p in sorted((ROOT / "scripts").rglob("*.sh")) if "pipefail" in p.read_text()]


def test_no_pipefail_script_pipes_into_a_quiet_grep():
    found = [
        f"{path.relative_to(ROOT)}:{number}: {line.strip()}"
        for path in pipefail_scripts()
        for number, line in enumerate(path.read_text().splitlines(), 1)
        if not line.lstrip().startswith("#") and PIPED_QUIET_GREP.search(line)
    ]
    assert not found, "a quiet grep at the end of a pipe under pipefail:\n" + "\n".join(found)


def test_the_pattern_tells_a_pipe_from_an_or():
    assert PIPED_QUIET_GREP.search("printf '%s' \"$x\" | grep -qi '^a:'")
    assert PIPED_QUIET_GREP.search("kubectl get cm -o yaml 2>/dev/null |grep -qF -- \"$v\"")
    assert not PIPED_QUIET_GREP.search("[ -n \"$a\" ] || grep -q 'x' \"$file\"")
    assert not PIPED_QUIET_GREP.search("printf '%s' \"$x\" | grep -c y")
