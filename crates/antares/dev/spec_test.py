#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""dev/spec.py's ARCHITECTURE.md rows and `sizes` on a throwaway tree (T-3762)."""

import contextlib
import io
import pathlib
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import spec  # noqa: E402

DOC = """\
# Map

| Crate | Owns |
|---|---|
| `antares-api` | the binding |
| `antares-store` | the drivers |

| Module | Surface |
|---|---|
| `lib.rs` | the router |
"""


def tree(root, doc=DOC):
    """A workspace of two crates: antares-api (lib.rs 1 200 lines, views.rs
    3 lines) and antares-store (150 lines)."""
    api = root / "crates/antares-api/src"
    store = root / "crates/antares-store/src"
    api.mkdir(parents=True)
    store.mkdir(parents=True)
    (api / "lib.rs").write_text("x\n" * 1200)
    (api / "views.rs").write_text("x\n" * 3)
    (store / "lib.rs").write_text("x\n" * 150)
    (root / "ARCHITECTURE.md").write_text(doc)


class Rows(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_a_missing_module_row_fails(self):
        tree(self.root)
        self.assertEqual(
            spec.architecture_row_violations(self.root),
            ["ARCHITECTURE.md: the module table has no row for views.rs"],
        )

    def test_a_missing_crate_row_fails(self):
        tree(self.root, DOC.replace("| `antares-store` | the drivers |\n", "") + "| `views.rs` | views |\n")
        self.assertEqual(
            spec.architecture_row_violations(self.root),
            ["ARCHITECTURE.md: the crate table has no row for antares-store"],
        )

    def test_rows_without_numbers_pass(self):
        tree(self.root, DOC + "| `views.rs` | views |\n")
        self.assertEqual(spec.architecture_row_violations(self.root), [])

    def test_sizes_prints_one_line_per_crate_and_module(self):
        tree(self.root)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            spec.cmd_sizes(self.root)
        self.assertEqual(
            out.getvalue().splitlines(),
            ["antares-api 1 200", "antares-store 150", "lib.rs 1 200", "views.rs 3"],
        )

    def test_sizes_on_the_workspace_exits_0(self):
        run = subprocess.run(
            [sys.executable, str(spec.ROOT / "dev/spec.py"), "sizes"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(len(run.stdout.splitlines()), len(spec.architecture_sizes()))

    def test_every_rounding_step(self):
        for n, want in [(0, "0"), (99, "99"), (104, "100"), (105, "110"), (1024, "1 000"), (55870, "55 900")]:
            self.assertEqual(spec.rounded(n), want, n)


if __name__ == "__main__":
    unittest.main()
