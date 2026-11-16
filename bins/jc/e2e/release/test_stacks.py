"""Unit checks of the release e2e harness: ports, the report fold, the coverage matcher (pytest)."""

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import coverage
import stacks


def test_ports_of_16_stacks_are_distinct_and_below_the_ephemeral_range():
    used = []
    for i in range(16):
        port, pg = stacks.ports(i)
        used += [port, port + 1, pg, pg + 100, pg + 200, pg + 300, pg + 301]
    assert len(used) == len(set(used))
    assert max(used) < 32768


def test_results_fold_a_retry_into_flaky_and_skip_the_lint_project(tmp_path):
    report = {"suites": [{"specs": [
        {"title": "J001 a", "tests": [{"projectName": "journeys", "results": [{"status": "passed", "duration": 5}]}]},
        {"title": "J002 b", "tests": [{"projectName": "journeys", "results": [
            {"status": "failed", "duration": 9}, {"status": "passed", "duration": 7}]}]},
        {"title": "J003 c", "tests": [{"projectName": "journeys", "results": [
            {"status": "failed", "duration": 9}, {"status": "failed", "duration": 8}]}]},
        {"title": "lint", "tests": [{"projectName": "lint", "results": [{"status": "passed", "duration": 1}]}]},
    ], "suites": []}]}
    f = tmp_path / "report.json"
    f.write_text(json.dumps(report))
    rows = stacks.results(f)
    assert rows == {"J001 a": {"status": "passed", "ms": 5}, "J002 b": {"status": "flaky", "ms": 7},
                    "J003 c": {"status": "failed", "ms": 8}}


def test_a_filled_page_covers_its_route_and_nothing_else():
    r = coverage.report({"journeys": [{"pages": ["/projects/helsinki/explore", "/catalogue"]}]},
                        ["/projects/$project/explore", "/catalogue", "/catalogue/$name", "/"])
    assert r["covered"] == ["/catalogue", "/projects/$project/explore"]
    assert r["uncovered"] == ["/catalogue/$name", "/"]
