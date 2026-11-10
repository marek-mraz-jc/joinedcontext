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

root by binary (built and run per partition): before the build, `--binaries` splits the targets
`cargo metadata` lists (every member's lib, bins and examples as one block, cargo's `--lib
--bins --examples` being workspace-wide; each integration test name, `--test x`, with the tests
of the serial groups whose filters name it kept in one partition) by recorded build and run
time, and prints this partition's cargo target arguments. A target whose tests alone would load
a partition past SPLIT of its share is built in two or more partitions and its tests are split
among them by name after the build (`--pieces`, which prints the filterset of the built tests:
every test of a whole target, this partition's piece of a split one).
  python3 test-part.py --binaries root K N [--extra K=S]      # cargo target arguments
  python3 test-part.py --pieces root K N --reuse BINARIES.json METADATA.json [--extra K=S]
  python3 test-part.py --plan root N [--extra K=S]            # the modelled partitions

  python3 test-part.py --record root|antares JUNIT.xml...   # rewrite that key from junit files
  python3 test-part.py --record portal LOG...               # from cargo test job logs
  python3 test-part.py --selftest
"""
import json
import math
import os
import re
import statistics
import subprocess
import sys

DURATIONS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "ci-durations.json")
# Portal, measured on ci runs 42756711313, 42760631366, 42767791074 and 42769151385 (build phase
# over target count, four partitions): one integration target compiles and links in about 2 s at
# -j 4; the lib test binary compiles for up to 276 s (cargo --timings, 42764897027) but beside the
# lib every partition builds, so it adds about 100 s, and its tests run about 25 s; `cargo test
# --doc` takes about 135 s when the lib changed.
PORTAL_BUILD = 2.0
PORTAL_LIB = 125.0
PORTAL_DOC = 135.0
# root by binary, from cargo --timings of root-test (runs 42764897027, 42769151385: 474 CPU
# seconds of build on 4 cores took 121 s, so wall time follows CPU time) and the build times of
# run 42823061257: the libs every partition builds (most from sccache) take about 100 CPU
# seconds, one integration test target compiles and links in about 2, the lib, bin and example
# test block in about 160 (46 lib and 7 bin test units, the bins); a partition holding a test of
# jc also builds jc's lib and bin, which link every module (about 120). The runner has 4 cores
# (`nproc`, ci.yml prints it).
ROOT_LIBS = 100.0
ROOT_TARGET = 2.0
ROOT_BLOCK = 160.0
ROOT_PACKAGE = {"jc": 120.0}
CORES = 4
THREADS = 4
# a target whose tests take more than this share of a partition's test time is split by test
SPLIT = 0.6
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


def filter_match(expr, pkg, binary):
    """Whether a nextest filter of package(), binary(), and, or, not and parentheses selects a test
    binary of pkg named binary. Any other predicate exits: it cannot be decided before the build."""
    toks = re.findall(r"\(|\)|&|\||!|[a-z_]+\([^()]*\)|[a-z]+", expr)
    if "".join(toks) != re.sub(r"\s", "", expr):
        sys.exit(f"filter this split cannot read: {expr}")
    pos = 0

    def peek():
        return toks[pos] if pos < len(toks) else None

    def take():
        nonlocal pos
        pos += 1
        return toks[pos - 1]

    def atom():
        tok = take()
        if tok == "(":
            v = any_()
            if take() != ")":
                sys.exit(f"unbalanced filter: {expr}")
            return v
        if tok in ("not", "!"):
            return not atom()
        m = re.fullmatch(r"(package|binary)\(=?([^()]*)\)", tok)
        if not m:
            sys.exit(f"filter predicate this split cannot decide before the build: {tok} in {expr}")
        return (pkg if m.group(1) == "package" else binary) == m.group(2)

    def all_():
        v = atom()
        while peek() in ("and", "&"):
            take()
            v = atom() and v
        return v

    def any_():
        v = all_()
        while peek() in ("or", "|"):
            take()
            v = all_() or v
        return v

    v = any_()
    if peek() is not None:
        sys.exit(f"filter this split cannot read: {expr}")
    return v


def root_targets(meta):
    """(the binary ids of the lib, bin and example test block, [(package, name)] of the
    integration test targets) of the workspace members in `cargo metadata` output."""
    members = set(meta["workspace_members"])
    block, tests = [], []
    for p in meta["packages"]:
        if p["id"] not in members:
            continue
        for t in p["targets"]:
            kind = set(t["kind"])
            if kind == {"custom-build"}:
                continue
            if kind & {"lib", "rlib", "proc-macro", "cdylib"} and t["test"]:
                block.append(p["name"])
            elif kind == {"bin"} and t["test"]:
                block.append(f"{p['name']}::bin/{t['name']}")
            elif kind == {"example"}:
                # `cargo test` builds an example to check it compiles; --examples builds it as a test
                block.append(f"{p['name']}::example/{t['name']}")
            elif kind == {"test"} and t["test"]:
                tests.append((p["name"], t["name"]))
            else:
                sys.exit(f"target this split does not know: {p['name']}:{t['name']} {t['kind']}")
    return block, tests


def root_plan(meta, rec, n, filters, extra):
    """The n partitions of the root workspace's targets: (parts, times). parts[k] lists items, each
    a dict with the target names it builds ("block": the lib, bin and example tests), its binary
    ids, and for a split target its piece and the partitions holding its pieces."""
    block, tests = root_targets(meta)
    sums, own = rec.get("sums", {}), rec.get("tests", {})
    known = [v for b, v in sums.items() if "::" in b and "::bin/" not in b]
    median = statistics.median(known) if known else 1.0

    def run(bid):
        return sums.get(bid, median)

    def longest(bid):
        mine = [v for k, v in own.items() if k.split(" ", 1)[0] == bid]
        return max(mine, default=rec.get("binaries", {}).get(bid, 0.0))

    def group(pkg, binary):
        return next((g for g, f in enumerate(filters) if filter_match(f, pkg, binary)), None)

    # integration test names joined when they share a serial group (`--test x` selects every
    # member's tests/x.rs, so one name is one item)
    names = sorted({x for _, x in tests})
    root = {x: x for x in names}

    def find(x):
        while root[x] != x:
            x = root[x]
        return x

    first = {}
    for pkg, x in tests:
        g = group(pkg, x)
        if g is not None:
            root[find(x)] = find(first.setdefault(g, x))
    items = {}
    for pkg, x in tests:
        it = items.setdefault(find(x), {"names": set(), "bids": [], "groups": {}, "pkgs": set()})
        it["names"].add(x)
        it["pkgs"].add(pkg)
        it["bids"].append(f"{pkg}::{x}")
        g = group(pkg, x)
        if g is not None:
            it["groups"][g] = it["groups"].get(g, 0.0) + run(f"{pkg}::{x}")
    items = list(items.values())
    for it in items:
        it["build"] = ROOT_TARGET * len(it["bids"])
    # the block builds every member's lib and bins
    lib = {"names": {"block"}, "bids": block, "build": ROOT_BLOCK, "groups": {}, "pkgs": set(ROOT_PACKAGE)}
    for bid in block:
        pkg = bid.split("::")[0]
        name = bid.split("/")[-1] if "/" in bid else pkg.replace("-", "_")
        g = group(pkg, name)
        if g is not None:
            lib["groups"][g] = lib["groups"].get(g, 0.0) + run(bid)
    items.append(lib)
    for it in items:
        it["run"] = sum(run(b) for b in it["bids"])
        it["longest"] = max((longest(b) for b in it["bids"]), default=0.0)
    share = sum(it["run"] for it in items) / THREADS / n
    pieces = []
    for it in items:
        m = 1
        if not it["groups"] and it is not lib:
            m = min(n, max(1, math.ceil(it["run"] / THREADS / (SPLIT * share))))
        for i in range(m):
            pieces.append(dict(it, run=it["run"] / m, piece=i, of=m))
    state = [{"build": 0.0, "run": 0.0, "groups": {}, "longest": 0.0, "pkgs": set(),
              "extra": extra.get(k + 1, 0.0)} for k in range(n)]

    def time(s):
        tests_ = max([s["run"] / THREADS, s["longest"]] + list(s["groups"].values()))
        return (ROOT_LIBS + s["build"]) / CORES + tests_ + s["extra"]

    def with_item(s, it):
        groups = dict(s["groups"])
        for g, v in it["groups"].items():
            groups[g] = groups.get(g, 0.0) + v
        pkg_build = sum(ROOT_PACKAGE.get(p, 0.0) for p in it["pkgs"] - s["pkgs"])
        return {"build": s["build"] + it["build"] + pkg_build, "run": s["run"] + it["run"], "groups": groups,
                "longest": max(s["longest"], it["longest"]), "pkgs": s["pkgs"] | it["pkgs"], "extra": s["extra"]}

    parts = [[] for _ in range(n)]
    key = lambda it: (-(it["build"] / CORES + it["run"] / THREADS), sorted(it["names"]), it["piece"])
    for it in sorted(pieces, key=key):
        # the pieces of one target go to different partitions
        free = [k for k in range(n) if not any(o["names"] == it["names"] for o in parts[k])]
        k = min(free, key=lambda i: (time(with_item(state[i], it)), state[i]["run"], i))
        state[k] = with_item(state[k], it)
        parts[k].append(it)
    for k in range(n):
        for it in parts[k]:
            it["holders"] = [j for j in range(n) if any(o["names"] == it["names"] for o in parts[j])]
    return parts, [round(time(s)) for s in state]


def root_args(part):
    """cargo target arguments that build exactly the targets of one partition."""
    if not part:
        sys.exit("empty partition: more partitions than targets")
    args = []
    for it in part:
        if it["names"] == {"block"}:
            args = ["--lib", "--bins", "--examples"] + args
        else:
            for x in sorted(it["names"]):
                args += ["--test", x]
    return args


def root_pieces(part, k, tests, rec):
    """The tests of partition k (0-based) out of the built tests: every test of a whole item, this
    partition's share of a split one (split by name and recorded time, the same in every holder)."""
    own, mean = rec.get("tests", {}), rec.get("binaries", {})
    bids = {b: it for it in part for b in it["bids"]}
    stray = sorted({b for b, _ in tests} - set(bids))
    if stray:
        sys.exit(f"built binaries no item of this partition names: {stray}")
    keep = []
    for it in part:
        mine = [t for t in tests if t[0] in it["bids"]]
        if it["of"] > 1:
            t = lambda x: own.get(f"{x[0]} {x[1]}", mean.get(x[0], 0.1))
            split = lpt(sorted(mine, key=lambda x: (-t(x), x)), it["of"], lambda load, x: load + t(x))
            mine = split[it["holders"].index(k)]
        keep += mine
    return keep


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
        sums = {}
        for (bid, _), secs in per.items():
            sums[bid] = sums.get(bid, 0.0) + statistics.mean(secs)
        data[key] = {"tests": own,
                     "binaries": {b: round(statistics.mean(v), 3) for b, v in sorted(light.items())},
                     # each binary's tests summed (the per-binary split weighs whole binaries)
                     "sums": {b: round(v, 1) for b, v in sorted(sums.items())}}
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
    # filters: the nextest.toml subset decides; a per-test predicate is refused
    f = "package(a-pg) or (package(srv) and (binary(cli) or binary(r)))"
    assert filter_match(f, "a-pg", "x") and filter_match(f, "srv", "cli") and not filter_match(f, "srv", "x")
    assert filter_match("package(p) & !binary(x)", "p", "y") and not filter_match("package(p) & !binary(x)", "p", "x")
    try:
        filter_match("test(slow)", "p", "x")
        raise AssertionError("test() accepted")
    except SystemExit:
        pass
    # root by binary: every target in exactly one partition but the split one, whose pieces
    # cover its tests once; the block once; names sharing a serial group together
    pkg = lambda name, targets: {"id": name, "name": name, "targets": [
        {"name": t, "kind": [k], "test": k != "example"} for k, t in targets]}
    meta = {"workspace_members": ["a-pg", "srv", "other", "big"], "packages": [
        pkg("a-pg", [("lib", "a_pg"), ("test", "store")]),
        pkg("srv", [("lib", "srv"), ("bin", "srv"), ("test", "cli"), ("test", "r"), ("test", "web")]),
        pkg("other", [("lib", "other"), ("test", "cli"), ("example", "demo")]),
        pkg("big", [("lib", "big"), ("test", "chat")] + [("test", f"t{i}") for i in range(9)])]}
    rec = {"tests": {f"big::chat c{i}": 10.0 for i in range(20)},
           "binaries": {"srv::web": 0.5},
           "sums": {"big::chat": 200.0, "srv::cli": 30.0, "other::cli": 5.0, "a-pg::store": 20.0,
                    **{f"big::t{i}": 8.0 for i in range(9)}}}
    parts, times = root_plan(meta, rec, 3, [f], {1: 10.0})
    args = [root_args(p) for p in parts]
    flat = [a for p in args for a in p]
    assert flat.count("--lib") == flat.count("--bins") == flat.count("--examples") == 1, args
    names = [a for a in flat if not a.startswith("--")]
    assert set(names) == {"chat", "cli", "r", "store", "web"} | {f"t{i}" for i in range(9)}, names
    assert len([x for x in names if x != "chat"]) == len(set(names)) - 1, names
    # cli (srv's in the group, other's by name), r and store (the same group) in one partition
    together = [p for p in args if "cli" in p]
    assert len(together) == 1 and "r" in together[0] and "store" in together[0], args
    # chat (200 s) is split; its pieces take each listed test once
    holders = [k for k, p in enumerate(args) if "chat" in p]
    assert len(holders) >= 2, args
    tests = [("big::chat", f"c{i}") for i in range(20)] + [("big::chat", "new")]
    got = []
    for k in holders:
        got += root_pieces(parts[k], k, tests, rec)
    assert sorted(got) == sorted(tests), got
    try:
        root_pieces(parts[holders[0]], holders[0], tests + [("stray::x", "y")], rec)
        raise AssertionError("stray binary accepted")
    except SystemExit:
        pass
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
    if args[:1] in (["--binaries"], ["--pieces"], ["--plan"]):
        mode = args[0]
        reuse, args = opts(args[1:], "--reuse", 2)
        extra, args = opts(args, "--extra")
        loads = {int(e.split("=")[0]): float(e.split("=")[1]) for e in extra}
        key, k, n = (args[0], 1, int(args[1])) if mode == "--plan" else (args[0], int(args[1]), int(args[2]))
        if not 1 <= k <= n or (mode == "--pieces") != (len(reuse) == 2):
            sys.exit(f"partition {k} of {n}, --reuse BINARIES METADATA with --pieces only?")
        meta = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--locked", "--format-version", "1"]))
        parts, times = root_plan(meta, load(key), n, serial_filters(), loads)
        if mode == "--plan":
            for j, (part, sec) in enumerate(zip(parts, times)):
                names = sorted(x if it["of"] == 1 else f"{x}[{it['piece'] + 1}/{it['of']}]"
                               for it in part for x in it["names"])
                print(f"{j + 1}: {sec} s, {len(names)} names: {' '.join(names)}")
        elif mode == "--binaries":
            print("\n".join(root_args(parts[k - 1])))
        else:
            tests = nextest_list(reuse, [])
            print("\n".join(filterset(root_pieces(parts[k - 1], k - 1, tests, load(key)), tests)))
        return
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


if __name__ == "__main__":
    main()
