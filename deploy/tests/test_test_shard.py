"""scripts/ci/test-shard.py, the time-balanced split of ci.yml's `tests` shards (T-4789)."""

import importlib.util
import itertools
import json
import os
import statistics
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/ci/test-shard.py"
spec = importlib.util.spec_from_file_location("test_shard", SCRIPT)
test_shard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(test_shard)


@pytest.fixture(autouse=True)
def every_file(monkeypatch):
    """A scoped ci shard sets JC_TEST_FILES for its whole job; these tests split every file."""
    monkeypatch.delenv("JC_TEST_FILES", raising=False)


def test_the_scoped_set_splits_only_its_own_files(monkeypatch):
    monkeypatch.setenv("JC_TEST_FILES", "tests/test_b.py tests/test_a.py")
    assert test_shard.files() == ["test_a.py", "test_b.py"]
    monkeypatch.setenv("JC_TEST_FILES", "")
    assert test_shard.files() == []
    monkeypatch.setenv("JC_TEST_FILES", "*")
    assert test_shard.files() == test_shard.all_files() and len(test_shard.files()) > 100
    printed = subprocess.run([sys.executable, str(SCRIPT), "1", "1"], check=True, capture_output=True, text=True,
                             env={**os.environ, "JC_TEST_FILES": "tests/test_test_shard.py"}).stdout.split()
    assert printed == ["tests/test_test_shard.py"]


def load(shard, weights):
    default = statistics.median(weights.values())
    return sum(weights.get(name, default) for name in shard)


def test_every_test_file_lands_in_exactly_one_shard():
    names = test_shard.files()
    printed = [
        subprocess.run([sys.executable, str(SCRIPT), str(i), "4"], check=True, capture_output=True, text=True).stdout.split()
        for i in range(1, 5)
    ]
    # ci.yml hands the printed paths to pytest in the repo root; bare names collected nothing (T-4791)
    assert all((test_shard.ROOT / path).is_file() for path in itertools.chain(*printed))
    # a split file runs in every shard, a quarter of its tests each (T-5886); any other in one
    cut = test_shard.split(names, json.loads(test_shard.TABLE.read_text(encoding="utf-8")), 4)
    assert all(sorted(cut) == sorted(Path(p).name for p in shard if Path(p).name in cut) for shard in printed)
    assert sorted(Path(path).name for path in itertools.chain(*printed) if Path(path).name not in cut) == sorted(set(names) - cut)
    assert all(printed)


def test_a_file_heavier_than_its_share_is_split_and_the_shards_stay_even():
    # test_smoke_script.py weighed 1,393 s against a 3,300 s share: whole, it filled most of one shard
    weights = {"test_heavy.py": 4000.0, **{f"test_{i:02}.py": 100.0 + i for i in range(40)}}
    names = sorted(weights)
    assert test_shard.split(names, weights, 4) == {"test_heavy.py"}
    shards = test_shard.shards(names, weights, 4)
    assert all("test_heavy.py" in s for s in shards)
    loads = [sum(weights[n] / (4 if n == "test_heavy.py" else 1) for n in s) for s in shards]
    loads[0] += test_shard.SHARD_ONE_STEPS
    assert max(loads) < 1.10 * min(loads), loads
    # the same weights whole: one shard holds the heavy file alone and the rest share three
    assert test_shard.split(names, weights, 1) == set()


def test_a_heavy_container_module_stays_whole_in_one_shard():
    # test_keycloak_admin_permissions.py runs its tests against one Keycloak (xdist_group): 785 s on 70de8827
    weights = {"test_keycloak_admin_permissions.py": 4000.0, **{f"test_{i:02}.py": 100.0 + i for i in range(40)}}
    names = sorted(weights)
    assert test_shard.split(names, weights, 6) == set()
    assert sum("test_keycloak_admin_permissions.py" in s for s in test_shard.shards(names, weights, 6)) == 1


def test_each_test_of_a_split_file_runs_in_exactly_one_shard():
    assert all(sum(test_shard.mine(position, shard, 4) for shard in range(1, 5)) == 1 for position in range(200))


def collected(*args, shard=None):
    env = {k: v for k, v in os.environ.items() if k != "JC_TEST_SHARD"} | ({"JC_TEST_SHARD": shard} if shard else {})
    out = subprocess.run([sys.executable, "-m", "pytest", "--collect-only", "-q", "-p", "no:cacheprovider", *args],
                         cwd=test_shard.ROOT, env=env, check=True, capture_output=True, text=True).stdout
    return [line for line in out.splitlines() if "::" in line]


def test_the_shards_collect_every_test_of_a_split_file_once():
    weights = json.loads(test_shard.TABLE.read_text(encoding="utf-8"))
    cut = sorted(test_shard.split(test_shard.files(), weights, 6))
    assert cut, "no file is heavy enough to split: pick one for this test"
    paths = [f"tests/{name}" for name in cut]
    whole = collected(*paths)
    parts = [collected(*paths, shard=f"{i}/6") for i in range(1, 7)]
    assert sorted(itertools.chain(*parts)) == sorted(whole) and all(parts)
    assert max(map(len, parts)) - min(map(len, parts)) <= len(cut)


def test_the_committed_table_spreads_the_time_evenly():
    # The positional split put 13.6 of 26 pytest minutes into shard 4 (run 39344805398).
    weights = json.loads(test_shard.TABLE.read_text(encoding="utf-8"))
    shards = test_shard.shards(test_shard.files(), weights, 6)
    loads = [load(s, weights) for s in shards]
    loads[0] += test_shard.SHARD_ONE_STEPS
    assert max(loads) <= 1.15 * statistics.mean(loads), loads


def test_a_file_the_table_lacks_weighs_the_median_and_still_runs_once(monkeypatch):
    # two files only: neither may count as heavy enough to split here
    monkeypatch.setattr(test_shard, "SPLIT", 1)
    weights = {"test_a.py": 1000.0, "test_b.py": 900.0}
    shards = test_shard.shards(["test_a.py", "test_b.py", "test_new.py"], weights, 2)
    assert sorted(itertools.chain(*shards)) == ["test_a.py", "test_b.py", "test_new.py"]
    # weighing the median (950), test_new goes second, onto shard 1 with its SHARD_ONE_STEPS;
    # weighing 0 it would go last and join test_a on shard 2
    assert shards == [["test_new.py"], ["test_a.py", "test_b.py"]]


def test_record_sums_setup_call_and_teardown_per_file(tmp_path):
    log = tmp_path / "pytest.log"
    log.write_text(
        "=== slowest durations ===\n"
        "84.18s call     test_apisix_edge.py::test_no_staging_objects_without_a_staging_domain\n"
        "41.62s setup    tests/test_context_brokers.py::test_the_policy_gate[x]\n"
        "1.00s teardown test_apisix_edge.py::test_other@group\n"
        "909 passed in 789.73s (0:13:09)\n",
        encoding="utf-8",
    )
    recorded = test_shard.record([log])
    assert recorded["test_apisix_edge.py"] == 85.2 and recorded["test_context_brokers.py"] == 41.6
    # every other test file ran only tests pytest hides (under 5 ms)
    assert sorted(recorded) == test_shard.files()
    assert round(sum(recorded.values()), 1) == 126.8


def test_every_test_file_has_a_measured_weight():
    # 73 files landed after the table was recorded and each weighed only the median (T-5551):
    # refresh it from the test-durations job of a main ci run (scripts/ci/test-shard.py --record)
    weights = json.loads(test_shard.TABLE.read_text(encoding="utf-8"))
    assert [name for name in test_shard.files() if name not in weights] == []
    assert sorted(weights) == test_shard.files(), "the table names a test file that is gone"


def test_shard_one_carries_less_pytest_time_for_its_own_steps():
    weights = json.loads(test_shard.TABLE.read_text(encoding="utf-8"))
    loads = [load(s, weights) for s in test_shard.shards(test_shard.files(), weights, 6)]
    assert test_shard.SHARD_ONE_STEPS >= 150 and all(loads[0] < other for other in loads[1:]), loads


def test_record_reads_the_shards_junit_reports_and_keeps_an_unmeasured_weight(tmp_path):
    reports = []
    for i, cases in enumerate((
        ('<testcase classname="tests.test_apisix_edge" name="a" time="80.5"/>'
         '<testcase classname="tests.test_apisix_edge.TestX" name="b" time="4.5"/>'),
        ('<testcase classname="tests.test_secrets" name="c" time="2.25"/>'
         '<testcase classname="tests.test_gone" name="d" time="9"/>'),
    )):
        reports.append(tmp_path / f"pytest-{i}.xml")
        reports[-1].write_text(f'<testsuites><testsuite name="pytest">{cases}</testsuite></testsuites>', encoding="utf-8")
    recorded = test_shard.record(reports, {"test_api_host.py": 13.4, "test_gone.py": 1.0})
    assert (recorded["test_apisix_edge.py"], recorded["test_secrets.py"]) == (85.0, 2.2)
    # a file neither report names keeps its weight (its shard did not finish); a deleted one drops out
    assert recorded["test_api_host.py"] == 13.4 and "test_gone.py" not in recorded
    assert sorted(recorded) == test_shard.files()


def test_record_refuses_reports_without_a_test(tmp_path):
    empty = tmp_path / "pytest-1.xml"
    empty.write_text("<testsuites/>", encoding="utf-8")
    proc = subprocess.run([sys.executable, str(SCRIPT), "--record", str(empty)], check=False, capture_output=True, text=True)
    assert proc.returncode != 0 and "no junit test cases" in proc.stderr


def test_a_shard_outside_the_count_is_refused():
    for argv in (["0", "4"], ["5", "4"], ["x", "4"], ["1"]):
        assert subprocess.run([sys.executable, str(SCRIPT), *argv], check=False, capture_output=True).returncode == 2
