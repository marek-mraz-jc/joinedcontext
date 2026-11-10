#!/usr/bin/env python3
"""Partition K of N of a workspace's tests, balanced by the times .github/ci-durations.json records.

Every mode assigns each test the list names to exactly one partition: a test or target the file
has no time for gets the median, so a new test joins a partition by itself. The times only move
tests between partitions, never in or out of the run.

Portal (cargo test; its database tests serialize on in-process locks, so binaries run one after
another): the integration test targets (every member's tests/*.rs, as `cargo metadata` lists
them), each weighed by its build plus run time; partition 1 also takes every lib and bin target.
A workspace with an example, a bench or a `test = false` target is refused: `cargo test`'s
default treats those differently from an explicit target flag. The doc tests run as their own
command (`cargo test --doc` refuses other target flags).
  python3 test-part.py K N [--doc K2]        # cargo test arguments, one per line

nextest (root, Antares): lists the tests of the built binaries (`--reuse`: the binaries-only
list and cargo metadata of the build, as `nextest run` takes them; `--filter`: only the tests
this selects) and prints this partition's filterset. Each partition's time is the larger of its
test time spread over the test threads and the time of its longest serial test group
(.config/nextest.toml groups with max-threads 1). `--extra K=SECONDS` loads a partition with a
step that runs beside or after it (the doc tests).
  python3 test-part.py --nextest root K N --reuse BINARIES.json METADATA.json [--filter E] [--extra K=S]
                                            # filterset terms, one per line (one -E each)

  python3 test-part.py --record root|antares JUNIT.xml...   # rewrite that key from junit files
  python3 test-part.py --record portal LOG...               # from cargo test job logs
  python3 test-part.py --selftest
"""
import json
import os
import re
import statistics
import subprocess
import sys

DURATIONS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "ci-durations.json")
# Portal, measured on ci runs 42756711313 and 42760631366 (logs: build phase over target count)
# and 42764897027 (cargo --timings): one integration target compiles and links in about 2 s at
# -j 4; the lib test binary alone compiles for 276 s (the other targets build beside it) and its
# tests run about 25 s; `cargo test --doc` about 135 s when the lib changed.
PORTAL_BUILD = 2.0
PORTAL_LIB = 300.0
PORTAL_DOC = 135.0
# a test with at least this many seconds keeps its own time in the file; the rest of a binary's
# tests are recorded as their mean (keeps the file small)
OWN_TIME = 2.0


def load(key):
    try:
        with open(DURATIONS) as f:
            return json.load(f).get(key, {})
    except FileNotFoundError:
        return {}


def lpt(items, n, cost, loads=None):
    """Longest first, each item to the partition it ends least loaded: a list of n lists.
    cost(loads_k, item) -> the partition's load with the item in it."""
    loads = list(loads or [0.0] * n)
    parts = [[] for _ in range(n)]
    for it in items:
        k = min(range(n), key=lambda i: (cost(loads[i], it), i))
        loads[k] = cost(loads[k], it)
        parts[k].append(it)
    return parts


def portal_split(tests, k, n, times, doc=None):
    """Partition k (1-based) of n: the libs and bins in 1, the tests by recorded time."""
    # by name: `--test x` selects every member's tests/x.rs, so one name goes to one partition
    names = sorted({name for _, name in tests})
    known = [times[x] for x in names if x in times]
    median = statistics.median(known) if known else 1.0
    weight = {x: PORTAL_BUILD + times.get(x, median) for x in names}
    loads = [0.0] * n
    loads[0] += PORTAL_LIB
    if doc:
        loads[doc - 1] += PORTAL_DOC
    items = sorted(names, key=lambda x: (-weight[x], x))
    parts = lpt(items, n, lambda load, x: load + weight[x], loads)
    args = ["--lib", "--bins"] if k == 1 else []
    for name in sorted(parts[k - 1]):
        args += ["--test", name]
    return args


def suites(listing):
    """(binary_id, test name) of every test a `nextest list --message-format json` output selects."""
    out = []
    for bid, suite in listing.get("rust-suites", {}).items():
        for name, case in suite.get("testcases", {}).items():
            if case.get("filter-match", {}).get("status") == "matches":
                out.append((bid, name))
    return sorted(out)


def nextest_split(tests, n, rec, serial_groups, threads, extra):
    """The n partitions (lists of (binary_id, name)) of tests by recorded time."""
    own, mean = rec.get("tests", {}), rec.get("binaries", {})
    known = list(own.values()) + list(mean.values())
    median = statistics.median(known) if known else 0.1

    def t(test):
        bid, name = test
        return own.get(f"{bid} {name}", mean.get(bid, median))

    group = {}
    for g, members in enumerate(serial_groups):
        for test in members:
            group[test] = g
    # a partition's state: total test seconds, seconds per serial group; time = the larger of the
    # total over the threads and the longest group (plus the extra step's seconds)
    state = [{"sum": 0.0, "groups": {}, "extra": extra.get(k + 1, 0.0)} for k in range(n)]

    def time(s):
        return s["extra"] + max([s["sum"] / threads] + list(s["groups"].values()))

    parts = [[] for _ in range(n)]
    for test in sorted(tests, key=lambda x: (-t(x), x)):
        g = group.get(test)

        def with_test(s):
            new = {"sum": s["sum"] + t(test), "groups": dict(s["groups"]), "extra": s["extra"]}
            if g is not None:
                new["groups"][g] = new["groups"].get(g, 0.0) + t(test)
            return new

        k = min(range(n), key=lambda i: (time(with_test(state[i])), state[i]["sum"], i))
        state[k] = with_test(state[k])
        parts[k].append(test)
    return parts


def filterset(part, tests, chunk=200):
    """nextest filterset terms (one -E each; nextest runs a test any of them selects) naming exactly
    the tests of part out of tests: a whole binary by id, else its tests by name, at most chunk names
    per term (Linux caps one argument at 128 KiB)."""
    mine, all_ = {}, {}
    for bid, name in tests:
        all_.setdefault(bid, set()).add(name)
    for bid, name in part:
        mine.setdefault(bid, set()).add(name)
    terms = []
    for bid in sorted(mine):
        if mine[bid] == all_[bid]:
            terms.append(f"binary_id(={bid})")
            continue
        names = sorted(mine[bid])
        for i in range(0, len(names), chunk):
            sel = " | ".join(f"test(={x})" for x in names[i:i + chunk])
            terms.append(f"binary_id(={bid}) & ({sel})")
    # an empty partition (more partitions than tests) selects nothing
    return terms or ["none()"]


def serial_filters(config=".config/nextest.toml"):
    """The filter of each test group with max-threads 1, from the overrides naming it."""
    import tomllib
    with open(config, "rb") as f:
        conf = tomllib.load(f)
    serial = [g for g, v in conf.get("test-groups", {}).items() if v.get("max-threads") == 1]
    out = []
    for g in serial:
        fs = [o["filter"] for prof in conf.get("profile", {}).values()
              for o in prof.get("overrides", []) if o.get("test-group") == g and "filter" in o]
        if fs:
            out.append(" | ".join(f"({x})" for x in fs))
    return out


def nextest_list(reuse, exprs):
    """The tests `cargo nextest list` of the built binaries selects (every expr must match)."""
    bins, meta = reuse
    cmd = ["cargo", "nextest", "list", "--profile", "ci", "--binaries-metadata", bins,
           "--cargo-metadata", meta, "--message-format", "json"]
    if exprs:
        cmd += ["-E", " & ".join(f"({e})" for e in exprs)]
    return suites(json.loads(subprocess.check_output(cmd)))


def record(key, paths):
    with open(DURATIONS) as f:
        data = json.load(f)
    if key == "portal":
        times = {}
        for path in paths:
            cur = None
            for line in open(path, errors="replace"):
                m = re.search(r"Running tests/(\S+)\.rs \(", line)
                if m:
                    cur = m.group(1)
                elif cur and (m := re.search(r"test result: .* finished in ([\d.]+)s", line)):
                    times.setdefault(cur, []).append(float(m.group(1)))
                    cur = None
        data["portal"] = {x: round(statistics.mean(v), 1) for x, v in sorted(times.items())}
    else:
        import xml.etree.ElementTree as ET
        per = {}
        for path in paths:
            for tc in ET.parse(path).getroot().iter("testcase"):
                per.setdefault((tc.get("classname"), tc.get("name")), []).append(float(tc.get("time", 0)))
        own, light = {}, {}
        for (bid, name), secs in sorted(per.items()):
            # the mean over the runs given: one run's runner can be 1.7x slower than another's
            sec = statistics.mean(secs)
            if sec >= OWN_TIME:
                own[f"{bid} {name}"] = round(sec, 1)
            else:
                light.setdefault(bid, []).append(sec)
        data[key] = {"tests": own,
                     "binaries": {b: round(statistics.mean(v), 3) for b, v in sorted(light.items())}}
    with open(DURATIONS, "w") as f:
        json.dump(data, f, indent=0, sort_keys=True)
        f.write("\n")


def selftest():
    # portal: every name once, libs in 1, the doc partition loaded, unknown names placed
    tests = [("a", "t%d" % i) for i in range(7)] + [("b", "t0")]
    times = {"t0": 300.0, "t1": 1.0, "t2": 1.0}
    parts = [portal_split(tests, k, 3, times, doc=2) for k in (1, 2, 3)]
    named = [a for p in parts for a in p if a not in ("--test", "--lib", "--bins")]
    assert sorted(named) == sorted({n for _, n in tests}), named
    assert parts[0][:2] == ["--lib", "--bins"] and "--lib" not in parts[1] + parts[2]
    # the 300 s target sits alone in the partition without the lib and the doc tests
    assert parts[2] == ["--test", "t0"], parts
    # nextest: every test exactly once, a serial group spread, unknown tests placed
    tests = [("x::it", "s%d" % i) for i in range(4)] + [("y", "p%d" % i) for i in range(8)] + [("new", "z")]
    rec = {"tests": {"x::it s0": 40.0, "x::it s1": 40.0, "x::it s2": 40.0, "x::it s3": 40.0},
           "binaries": {"y": 10.0}}
    serial = [[t for t in tests if t[0] == "x::it"]]
    parts = nextest_split(tests, 2, rec, serial, 4, {1: 20.0})
    assert sorted(t for p in parts for t in p) == sorted(tests)
    assert [len([t for t in p if t[0] == "x::it"]) for p in parts] == [2, 2], parts
    # the filtersets cover every test and only those: a whole binary by id, a part by name
    terms = [filterset(p, tests) for p in parts]
    assert all("binary_id(=new)" in t or ("new", "z") not in p for t, p in zip(terms, parts))
    assert any(x.startswith("binary_id(=x::it) & (test(=s") for x in terms[0]), terms[0]
    assert filterset([], tests) == ["none()"]
    # a long binary is cut into terms of at most chunk names
    many = [("big", "t%03d" % i) for i in range(5)] + [("other", "o")]
    cut = filterset(many[:5], many + [("big", "u")], chunk=2)
    assert len(cut) == 3 and all(x.count("test(=") <= 2 for x in cut), cut
    print("test-part.py selftest ok")


def opts(args, name, nargs=1):
    """Values of every `name V...` (nargs values) in args, and args without them."""
    vals, rest, i = [], [], 0
    while i < len(args):
        if args[i] == name:
            vals += args[i + 1:i + 1 + nargs]
            i += 1 + nargs
        else:
            rest.append(args[i])
            i += 1
    return vals, rest


def main():
    args = sys.argv[1:]
    if args == ["--selftest"]:
        return selftest()
    if args[:1] == ["--record"]:
        return record(args[1], args[2:])
    if args[:1] == ["--nextest"]:
        reuse, args = opts(args[1:], "--reuse", 2)
        base, args = opts(args, "--filter")
        extra, args = opts(args, "--extra")
        key, k, n = args[0], int(args[1]), int(args[2])
        if not 1 <= k <= n or len(reuse) != 2:
            sys.exit(f"partition {k} of {n}, --reuse BINARIES METADATA?")
        tests = nextest_list(reuse, base)
        groups = [nextest_list(reuse, base + [g]) for g in serial_filters()]
        loads = {int(e.split("=")[0]): float(e.split("=")[1]) for e in extra}
        parts = nextest_split(tests, n, load(key), groups, 4, loads)
        print("\n".join(filterset(parts[k - 1], tests)))
        return
    doc, args = opts(args, "--doc")
    k, n = int(args[0]), int(args[1])
    if not 1 <= k <= n:
        sys.exit(f"partition {k} of {n}?")
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--locked", "--format-version", "1"]))
    members = set(meta["workspace_members"])
    targets = [(p["name"], t) for p in meta["packages"] if p["id"] in members for t in p["targets"]]
    odd = [f"{p}:{t['name']}" for p, t in targets
           if not set(t["kind"]) <= {"lib", "bin", "test", "custom-build", "proc-macro"}
           or (t["kind"] != ["custom-build"] and not t["test"])]
    if odd:
        sys.exit(f"targets this split does not know: {odd}")
    tests = [(p, t["name"]) for p, t in targets if "test" in t["kind"]]
    if not tests:
        sys.exit("no test targets")
    print("\n".join(portal_split(tests, k, n, load("portal"), int(doc[0]) if doc else None)))


main()
