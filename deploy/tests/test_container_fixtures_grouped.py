"""A test module that starts a named container keeps its tests on one xdist worker (T-3897).

CI runs `pytest -n auto --dist loadgroup`: a module without an `xdist_group` mark has its tests
spread over the workers, and each worker sets up a module-scoped fixture of its own. Two of them
starting the same named container at once is a `docker run` exit 125 ("name already in use"),
which turned deployment ci red at 9f268a8 (run 38176256389, `t2725-kc-{seconds}`), and two
Keycloaks at once is memory the runner does not have to spare.
"""

import re
from pathlib import Path

TESTS = Path(__file__).parent
STARTS_NAMED_CONTAINER = re.compile(r'"docker",\s*"run"[^\]]*"--name"', re.DOTALL)


def test_every_module_that_starts_a_named_container_is_one_xdist_group():
    ungrouped = [
        path.name
        for path in sorted(TESTS.glob("test_*.py"))
        if STARTS_NAMED_CONTAINER.search(path.read_text(encoding="utf-8"))
        and "xdist_group" not in path.read_text(encoding="utf-8")
    ]
    assert not ungrouped, f"named containers started from several workers: {ungrouped}"


def test_the_check_sees_a_named_container():
    assert STARTS_NAMED_CONTAINER.search('["docker", "run", "-d", "--name", name, "-P"]')
    assert not STARTS_NAMED_CONTAINER.search('run(PACKAGE, bin_dir, "--name", "test-archive")')
