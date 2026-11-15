"""ci/merge.py: the shards of the smoke gate fold into the verdict one run of every suite gives."""
import importlib.util
import json
import shutil
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


merge = load("smoke_ci_merge", HERE / "merge.py")
all_suite = merge.all_suite

# A suite whose rows follow its flags: --part a|b splits it as the gate splits ui by persona.
SPLIT = '''
import argparse
from pathlib import Path
from common import Results
def main(argv):
    p = argparse.ArgumentParser()
    p.add_argument("--base-url"); p.add_argument("--out"); p.add_argument("--part", default="a,b")
    a = p.parse_args(argv)
    r = Results("split", a.base_url)
    for part in a.part.split(","):
        r.check(f"walk:{part}", "t", "GET /", "200", "200", True)
    r.write(Path(a.out))
    return 0
'''
ONE = '''
import argparse, os
from pathlib import Path
from common import Results
def main(argv):
    p = argparse.ArgumentParser()
    p.add_argument("--base-url"); p.add_argument("--out")
    a = p.parse_args(argv)
    r = Results("one", a.base_url)
    r.check("x", "t", "GET /x", "200", "200", os.environ.get("SMOKE_TEST_RED") != "1")
    r.check("x", "t", "GET /x again", "200", "200", True)
    r.write(Path(a.out))
    return 0
'''
LOGS = '''
import argparse, os
from pathlib import Path
from common import Results
def main(argv):
    p = argparse.ArgumentParser()
    p.add_argument("--base-url"); p.add_argument("--out"); p.add_argument("--results-dir", action="append", default=[])
    a = p.parse_args(argv)
    r = Results("logs", a.base_url)
    leak = os.environ.get("SMOKE_TEST_LEAK") == Path(a.out).parent.name
    r.check("secrets", "t", "scan", "no secret", "leak" if leak else "none", not leak, "a token in the log" if leak else "")
    r.write(Path(a.out))
    return 0
'''


def smoke(tmp: Path) -> Path:
    root = tmp / "smoke"
    root.mkdir()
    for f in ("run", "common.py", "live.py", "second_factor.py"):
        shutil.copy(all_suite.SMOKE / f, root / f)
    (root / "all").mkdir()
    for name, text in {"split": SPLIT, "one": ONE, "logs": LOGS}.items():
        (root / name).mkdir()
        (root / name / "suite.py").write_text(text)
    return root


def run_all(out: Path, *extra) -> int:
    return all_suite.main(["--base-url", "http://127.0.0.1:1", "--out", str(out), *extra])


def shards(tmp: Path, names=("s1", "s2")) -> list[Path]:
    """The gate's split: s1 runs one and split's part a, s2 split's part b; logs on both."""
    flags = {"s1": ["--only", "one,split,logs", "--with", "split=--part a"],
             "s2": ["--only", "split,logs", "--with", "split=--part b"]}
    dirs = []
    for n in names:
        run_all(tmp / n, "--shard", n, *flags[n])
        dirs.append(tmp / n)
    return dirs


def doc(out: Path) -> dict:
    return json.loads((out / "results.json").read_text())


def test_the_shards_hold_the_rows_and_verdict_of_one_run(tmp_path, monkeypatch):
    monkeypatch.setattr(all_suite, "SMOKE", smoke(tmp_path))
    assert run_all(tmp_path / "whole") == 0
    code = merge.main(["--out", str(tmp_path / "m"), "--expect", "s1,s2", *map(str, shards(tmp_path))])
    whole, merged = doc(tmp_path / "whole"), doc(tmp_path / "m")
    ids = lambda d: Counter(r["id"] for r in d["checks"])
    # the one-run e2e placeholder is the only row a shard (no --e2e) leaves out
    assert ids(merged) == ids(whole) - Counter(["e2e/cargo"])
    assert code == 0 and merged["verdict"] == whole["verdict"] == "GREEN"
    assert [s["suite"] for s in merged["suites"]] == ["one", "split", "logs"]
    report = (tmp_path / "m" / "REPORT.md").read_text()
    assert "(0 not on the known list)" in report and "Merged from 2 of 2 shards" in report
    assert "../s1/split/REPORT.md, ../s2/split/REPORT.md" in report


def test_a_failure_on_one_shard_turns_the_merge_red_and_logs_fold_to_the_worst(tmp_path, monkeypatch):
    monkeypatch.setattr(all_suite, "SMOKE", smoke(tmp_path))
    monkeypatch.setenv("SMOKE_TEST_LEAK", "s2")
    code = merge.main(["--out", str(tmp_path / "m"), "--expect", "s1,s2", *map(str, shards(tmp_path))])
    m = doc(tmp_path / "m")
    logs = [r for r in m["checks"] if r["id"] == "logs/secrets"]
    assert len(logs) == 1 and logs[0]["result"] == "fail" and logs[0]["reason"] == "shard s2: a token in the log"
    assert code == 1 and m["verdict"] == "RED" and m["newFailures"] == 1


def test_the_known_list_is_applied_to_the_merged_rows(tmp_path, monkeypatch):
    monkeypatch.setattr(all_suite, "SMOKE", smoke(tmp_path))
    monkeypatch.setenv("SMOKE_TEST_RED", "1")
    known = tmp_path / "known.txt"
    known.write_text("^one/x$  # filed as T-1, worker\n^gone/  # fixed long ago\n")
    code = merge.main(["--out", str(tmp_path / "m"), "--expect", "s1,s2", "--known", str(known),
                       *map(str, shards(tmp_path))])
    m = doc(tmp_path / "m")
    assert code == 0 and m["verdict"] == "GREEN" and m["knownFixed"] == ["^gone/"]
    assert "1 fail (0 not on the known list)" in (tmp_path / "m" / "REPORT.md").read_text()


def test_a_missing_shard_is_red(tmp_path, monkeypatch):
    monkeypatch.setattr(all_suite, "SMOKE", smoke(tmp_path))
    dirs = shards(tmp_path, ("s1",))
    code = merge.main(["--out", str(tmp_path / "m"), "--expect", "s1,s2", *map(str, dirs), str(tmp_path / "s2")])
    m = doc(tmp_path / "m")
    assert code == 1 and m["verdict"] == "RED"
    assert [r["id"] for r in m["checks"] if r["result"] == "fail"] == ["shards/missing:s2"]


def test_a_shard_outside_expect_and_a_run_no_shard_ran_are_red(tmp_path, monkeypatch):
    monkeypatch.setattr(all_suite, "SMOKE", smoke(tmp_path))
    run_all(tmp_path / "s1", "--shard", "s1", "--only", "one")
    code = merge.main(["--out", str(tmp_path / "m"), "--expect", "s0", str(tmp_path / "s1")])
    fails = {r["id"] for r in doc(tmp_path / "m")["checks"] if r["result"] == "fail"}
    assert code == 1
    assert fails == {"shards/missing:s0", "shards/unexpected:s1", "shards/not-run:split", "shards/not-run:logs"}


def test_a_split_suite_row_from_two_shards_is_red(tmp_path, monkeypatch):
    monkeypatch.setattr(all_suite, "SMOKE", smoke(tmp_path))
    for n in ("s1", "s2"):  # both run part a: walk:a would count twice
        run_all(tmp_path / n, "--shard", n, "--with", "split=--part a", "--only", "split,logs" if n == "s2" else "one,split,logs")
    code = merge.main(["--out", str(tmp_path / "m"), "--expect", "s1,s2", str(tmp_path / "s1"), str(tmp_path / "s2")])
    fails = [r for r in doc(tmp_path / "m")["checks"] if r["result"] == "fail"]
    assert code == 1 and [r["id"] for r in fails] == ["shards/twice:split/walk:a"] and fails[0]["actual"] == "s1, s2"
