"""Every download a workflow makes fails loudly and retries (T-4101, TS-19).

A bare `curl -sSL` writes GitHub's error page into the tarball on a transient 5xx; the checksum
check after it then fails with `--status` and nothing in the log (run 38265103482). `--fail`
turns the answer into curl's own error, `--retry 3 --retry-all-errors` rides out the blip."""

import re
from pathlib import Path

WORKFLOWS = Path(__file__).resolve().parent.parent / ".github/workflows"
CURL = re.compile(r"(?:^|[^-\w])curl\s(.*)")


def test_every_workflow_download_fails_loudly_and_retries():
    found = 0
    for path in sorted(WORKFLOWS.glob("*.yml")):
        for number, line in enumerate(path.read_text().splitlines(), 1):
            match = CURL.search(line)
            if not match or line.lstrip().startswith("#"):
                continue
            found += 1
            flags = match.group(1)
            assert "--fail" in flags, f"{path.name}:{number} downloads without --fail"
            assert "--retry " in flags and "--retry-all-errors" in flags, f"{path.name}:{number} does not retry"
    assert found >= 20


NPX = re.compile(r"(?:^|[^-\w])npx\s(.*)")


def test_every_workflow_npx_resolves_only_cooled_down_releases():
    """npx resolves transitive ranges afresh: without --before it took a release published 80 s
    earlier that the registry did not serve yet, and the renovate job 404ed (run 39658420429)."""
    found = 0
    for path in sorted(WORKFLOWS.glob("*.yml")):
        for number, line in enumerate(path.read_text().splitlines(), 1):
            match = NPX.search(line)
            if not match or line.lstrip().startswith("#"):
                continue
            found += 1
            assert "--before=\"$(date -u -d '7 days ago'" in match.group(1), f"{path.name}:{number} npx without the 7-day --before"
    assert found >= 1
