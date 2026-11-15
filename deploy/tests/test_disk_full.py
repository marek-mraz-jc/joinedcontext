"""scripts/disk-full.py, the dev-smoke check of the postgres data volume (T-4414, OPS-27)."""

import subprocess
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/disk-full.py"

DF = """Filesystem     1024-blocks      Used Available Capacity Mounted on
/dev/sda3        320856064 {used} 60000000      {pct}% /var/lib/postgresql/data
"""


def run(stdin, limit=85):
    return subprocess.run([sys.executable, str(SCRIPT), "--max", str(limit), "--label", "postgres data"],
                          input=stdin, capture_output=True, text=True, check=False)


def test_a_volume_at_or_under_the_limit_passes_silently():
    for pct in (74, 85):
        out = run(DF.format(used=1, pct=pct))
        assert (out.returncode, out.stdout) == (0, ""), pct


def test_a_volume_over_the_limit_fails_with_its_mount_and_reading():
    out = run(DF.format(used=1, pct=86))
    assert out.returncode == 1
    assert out.stdout.strip() == "FAIL  postgres data (/var/lib/postgresql/data) is 86 % full, above 85 %"


def test_no_reading_is_a_failure_never_a_pass():
    for stdin in ("", "error: unable to upgrade connection: container not found\n", "Filesystem 1024-blocks Used\n"):
        out = run(stdin)
        assert out.returncode == 1, stdin
        assert "no df reading" in out.stdout
