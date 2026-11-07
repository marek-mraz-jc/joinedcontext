#!/usr/bin/env python3
"""Self-check for the ETSI resource-sampling pair and the per-TP baseline gate.

The two pieces with real logic in them: the sampler's /proc parsing, and the
report's sample↔TP interval join. Both fail silently if they break — an empty
CSV column looks exactly like "nothing happened" — so they get one assert-based
check. std-only, no framework.

Run: python3 dev/etsi-selftest.py
"""
import csv
import json
import os
import subprocess
import sys
import tempfile
import urllib.request
from datetime import datetime

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import importlib.util

spec = importlib.util.spec_from_file_location("sampler", f"{HERE}/etsi-sampler.py")
sampler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sampler)


def check_proc_parsing():
    """cpu_ticks/rss_mib must read THIS process, and CPU must advance."""
    me = os.getpid()
    rss = sampler.rss_mib(me)
    assert rss and rss > 0, f"rss_mib({me}) returned {rss!r} — /proc parsing broke"

    t0 = sampler.cpu_ticks(me)
    assert t0 is not None, "cpu_ticks returned None for a live process"
    x = 0
    for i in range(3_000_000):  # burn measurable CPU
        x += i
    t1 = sampler.cpu_ticks(me)
    assert t1 > t0, f"cpu ticks did not advance across real work ({t0} -> {t1})"

    assert sampler.cpu_ticks(2**30) is None, "a dead pid must yield None, not raise"
    assert sampler.rss_mib(2**30) is None, "a dead pid must yield None, not raise"
    print(f"  proc parsing ok (self rss {rss:.0f} MiB, ticks {t0}->{t1})")


def check_join():
    """A sample inside a TP gets that TP; one in the gap gets (between tests);
    one outside the suite entirely stays unlabelled."""
    base = datetime(2026, 8, 4, 12, 0, 0)
    t0 = base.timestamp()

    def iso(offset):
        return datetime.fromtimestamp(t0 + offset).strftime("%Y-%m-%dT%H:%M:%S.%f")

    with tempfile.TemporaryDirectory() as tmp:
        os.mkdir(f"{tmp}/DemoSuite")
        with open(f"{tmp}/DemoSuite/output.xml", "w") as fh:
            fh.write(f"""<?xml version="1.0"?>
<robot>
<suite id="s1" name="DemoSuite">
  <test id="s1-t1" name="001_01 First"><status status="PASS" start="{iso(0)}" elapsed="2.0"/></test>
  <test id="s1-t2" name="002_01 Second"><status status="FAIL" start="{iso(10)}" elapsed="2.0">boom went the payload</status></test>
  <status status="FAIL" start="{iso(0)}" elapsed="12.0"/>
</suite>
<statistics><total><stat pass="1" fail="1" skip="0">All Tests</stat></total></statistics>
</robot>""")

        with open(f"{tmp}/resource-samples.csv", "w") as fh:
            fh.write("ts,iso,container,phase,cpu_pct,rss_mib\n")
            for offset in (1, 6, 11, 500):  # in t1, gap, in t2, far outside
                fh.write(f"{t0 + offset:.0f},{iso(offset)[:19]},antares1,DemoSuite,10.0,100.0\n")

        env = {**os.environ, "RESULTS": tmp, "STORE": "memory",
               "MEM_LIMIT_MB": "350", "IMAGE_BYTES": "0"}
        subprocess.run([sys.executable, f"{HERE}/etsi-report.py"],
                       env=env, check=True, capture_output=True)

        got = {int(float(r["ts"]) - t0): r["test"]
               for r in csv.DictReader(open(f"{tmp}/resource-samples.csv"))}
        assert got[1] == "001_01 First", f"in-test sample mislabelled: {got[1]!r}"
        assert got[11] == "002_01 Second", f"in-test sample mislabelled: {got[11]!r}"
        assert got[6] == "(between tests)", f"gap sample mislabelled: {got[6]!r}"
        assert got[500] == "", f"sample outside the run must stay unlabelled: {got[500]!r}"

        fails = list(csv.DictReader(open(f"{tmp}/failures.csv")))
        assert len(fails) == 1 and fails[0]["test"] == "002_01 Second", fails
        assert "boom went the payload" in fails[0]["message"], fails

        rollup = list(csv.DictReader(open(f"{tmp}/resource-by-test.csv")))
        assert any(r["test"] == "001_01 First" and r["samples"] == "1" for r in rollup), rollup

        tps = {r["test"]: r["status"] for r in csv.DictReader(open(f"{tmp}/tp-status.csv"))}
        assert tps == {"001_01 First": "PASS", "002_01 Second": "FAIL"}, tps

        summary = open(f"{tmp}/run-summary.md").read()
        assert "002_01 Second" in summary, "the failure must reach the summary"
        assert open(f"{tmp}/gate-status.txt").read().strip() == "FAIL", \
            "a failing suite must fail the gate"
        print(f"  join ok ({len(got)} samples labelled, gate FAIL on a red suite)")


def check_baseline():
    """dev/etsi-baseline.py: a TP that flips red or stops running fails the
    gate and is named; a new passing TP only asks for a baseline update."""
    tps = ["TP/a.robot::001_01 First", "TP/a.robot::002_01 Second"]
    with tempfile.TemporaryDirectory() as tmp:
        base = f"{tmp}/baseline.json"
        with open(base, "w") as fh:
            json.dump({"tps": tps, "mqtt_tps": tps[1:]}, fh)

        def gate(rows, stores="memory", mqtt_off="", expected=""):
            cell = f"{tmp}/cells/ETSI-cell-memory"
            os.makedirs(cell, exist_ok=True)
            with open(f"{cell}/tp-status.csv", "w", newline="") as fh:
                w = csv.writer(fh)
                w.writerow(["suite", "source", "test", "status"])
                for key, status in rows:
                    source, name = key.split("::")
                    w.writerow(["Demo", source, name, status])
            env = {**os.environ, "BASELINE": base, "STORES": stores, "MQTT_OFF": mqtt_off}
            if expected:
                with open(f"{tmp}/expected", "w") as fh:
                    fh.write(expected)
                env["EXPECTED"] = f"{tmp}/expected"
            return subprocess.run([sys.executable, f"{HERE}/etsi-baseline.py", "check",
                                   f"{tmp}/cells"], env=env, capture_output=True, text=True)

        green = gate([(t, "PASS") for t in tps])
        assert green.returncode == 0, green.stdout
        flipped = gate([(tps[0], "PASS"), (tps[1], "FAIL")])
        assert flipped.returncode != 0, "a flipped TP must fail the gate"
        assert "FAIL: TP/a.robot::002_01 Second" in flipped.stdout, flipped.stdout
        gone = gate([(tps[0], "PASS")])
        assert gone.returncode != 0, "a TP that stopped running must fail the gate"
        assert "did not run: TP/a.robot::002_01 Second" in gone.stdout, gone.stdout
        new = gate([(t, "PASS") for t in tps] + [("TP/b.robot::003_01 Third", "PASS")])
        assert new.returncode == 0, new.stdout
        assert "ratchet: update baseline: TP/b.robot::003_01 Third" in new.stdout, new.stdout
        # a cell running with MQTT=0 may lack the MQTT TPs, and only those
        assert gate([(tps[0], "PASS")], mqtt_off="memory").returncode == 0
        assert gate([(tps[1], "PASS")], mqtt_off="memory").returncode != 0
        absent = gate([(t, "PASS") for t in tps], stores="memory file")
        assert absent.returncode != 0 and "file: no tp-status.csv" in absent.stdout, absent.stdout
        # a policy cell (EXPECTED): a listed refusal is green, the same TP passing is red
        listed = "# refused by the restricted policy\n002_01  5.6.1 refused\n"
        refused = gate([(tps[0], "PASS"), (tps[1], "FAIL")], expected=listed)
        assert refused.returncode == 0, refused.stdout
        let_through = gate([(t, "PASS") for t in tps], expected=listed)
        assert let_through.returncode != 0, "a refusal that did not happen must fail the gate"
        assert "expected to fail and did not: 002_01 (5.6.1 refused)" in let_through.stdout, let_through.stdout
        other = gate([(tps[0], "FAIL"), (tps[1], "FAIL")], expected=listed)
        assert other.returncode != 0 and "FAIL: TP/a.robot::001_01 First" in other.stdout, other.stdout
    print("  baseline ok (flip and vanish fail, a new pass asks for a ratchet)")


def check_compare():
    """dev/etsi-baseline.py compare: a TP whose verdict differs between the
    vanilla and the full run, or that ran in one only, fails and is named;
    the same failure in both is not a difference; an empty run fails."""
    with tempfile.TemporaryDirectory() as tmp:

        def run(name, rows):
            d = f"{tmp}/{name}"
            os.makedirs(d, exist_ok=True)
            with open(f"{d}/tp-status.csv", "w", newline="") as fh:
                w = csv.writer(fh)
                w.writerow(["suite", "source", "test", "status"])
                for key, status in rows:
                    source, test = key.split("::")
                    w.writerow(["IOP", source, test, status])
            return d

        def compare(a, b):
            return subprocess.run([sys.executable, f"{HERE}/etsi-baseline.py", "compare", a, b],
                                  capture_output=True, text=True)

        one, two = "IOP_TP/a.robot::IOP_001 One", "IOP_TP/a.robot::IOP_002 Two"
        vanilla = run("vanilla", [(one, "PASS"), (two, "FAIL")])
        same = compare(vanilla, run("same", [(one, "PASS"), (two, "FAIL")]))
        assert same.returncode == 0, same.stdout
        flipped = compare(vanilla, run("flipped", [(one, "FAIL"), (two, "FAIL")]))
        assert flipped.returncode != 0 and f"vanilla PASS, full FAIL: {one}" in flipped.stdout, flipped.stdout
        gone = compare(vanilla, run("gone", [(two, "FAIL")]))
        assert gone.returncode != 0 and f"vanilla PASS, full did not run: {one}" in gone.stdout, gone.stdout
        empty = compare(vanilla, run("empty", []))
        assert empty.returncode != 0 and "no TP ran" in empty.stdout, empty.stdout
        missing = compare(vanilla, f"{tmp}/nowhere")
        assert missing.returncode != 0, missing.stdout
    print("  compare ok (a flip or a TP in one run only fails, a shared failure does not)")


def check_view_diff():
    """dev/etsi-view-diff.py: a TP that differs through the view passes only
    when its body sent a request the view does not offer, an entity map or a
    Tenant, or the known file names it; a known TP that answers as E1 again,
    a deviation nobody owes and a TP that did not run fail."""
    vd = importlib.util.spec_from_file_location("view_diff", f"{HERE}/etsi-view-diff.py")
    view_diff = importlib.util.module_from_spec(vd)
    vd.loader.exec_module(view_diff)
    binds = view_diff.bindings()
    root = "http://h/x/views/v/ngsi-ld/v1"
    for method, url, op in [
        ("GET", f"{root}/entities/urn:a", "retrieve_entity"),
        ("GET", f"{root}/types?details=true", "retrieve_entity_type_details"),
        ("GET", f"{root}/types", "retrieve_entity_types"),
        ("GET", f"{root}/entities/urn:a/attrs/p/value", "retrieve_attribute_value"),
        ("POST", f"{root}/entityOperations/upsert?options=update", "batch_upsert"),
        ("POST", f"{root}/entities/", "create_entity"),
        ("GET", f"{root}/nothing", None),
        ("GET", "http://h/q/health", None),
    ]:
        assert view_diff.operation(method, url, binds) == op, (method, url)

    a, b, c, d = (f"TP/s.robot::00{i} T" for i in range(4))
    with tempfile.TemporaryDirectory() as tmp:

        def run(name, rows, requests=()):
            path = f"{tmp}/{name}"
            os.makedirs(path)
            with open(f"{path}/tp-status.csv", "w", newline="") as fh:
                w = csv.writer(fh)
                w.writerow(["suite", "source", "test", "status"])
                for key, status in rows:
                    w.writerow(["s", *key.split("::"), status])
            with open(f"{path}/requests.jsonl", "w") as fh:
                # a fixture's request has no test and names its suite file
                for test, method, url, headers, *extra in requests:
                    line = {"test": test, "method": method, "url": url, "headers": headers}
                    if test is None:
                        line.update(fixture=True, suite=extra[0])
                        if len(extra) > 1:
                            line["body"] = extra[1]
                    elif extra:
                        line["body"] = extra[0]
                    fh.write(json.dumps(line) + "\n")
            return path

        view = f"{tmp}/view.json"
        json.dump({"id": "v", "operations": ["retrieve_entity", "create_entity"]}, open(view, "w"))
        known = f"{tmp}/known.json"
        json.dump({"v": {d: "T-0000"}}, open(known, "w"))
        view_diff.KNOWN = known
        e1 = run("e1", [(a, "PASS"), (b, "PASS"), (c, "PASS"), (d, "PASS")])
        owed = run("owed", [(a, "FAIL"), (b, "FAIL"), (c, "FAIL"), (d, "FAIL")], [
            (a, "POST", f"{root}/entityOperations/upsert", []),
            (b, "GET", f"{root}/entities/urn:x?entityMap=true", []),
            (c, "GET", f"{root}/entities/urn:x", ["ngsild-tenant"]),
        ])
        assert view_diff.diff(view, e1, owed, binds) == 0
        refusals = json.load(open(f"{owed}/owed-refusals.json"))["refusals"]
        assert [r["tp"] for r in refusals] == [a, b, c], refusals
        stale = run("stale", [(a, "PASS"), (b, "PASS"), (c, "PASS"), (d, "PASS")])
        assert view_diff.diff(view, e1, stale, binds) == 1, "a known TP answering as E1 must fail"
        offered = run("offered", [(a, "FAIL"), (b, "PASS"), (c, "PASS"), (d, "FAIL")],
                      [(a, "POST", f"{root}/entities", [])])
        assert view_diff.diff(view, e1, offered, binds) == 1, "an offered operation owes nothing"
        gone = run("gone", [(b, "PASS"), (c, "PASS"), (d, "FAIL")])
        assert view_diff.diff(view, e1, gone, binds) == 1, "a TP that did not run must fail"
        json.dump({"id": "v", "operations": ["create_subscription", "retrieve_subscription"]}, open(view, "w"))
        sub = f"{root}/subscriptions/urn:ngsi-ld:Subscription:1"
        rootsub = run("rootsub", [(a, "FAIL"), (b, "PASS"), (c, "PASS"), (d, "FAIL")],
                      [(a, "GET", sub, [])])
        assert view_diff.diff(view, e1, rootsub, binds) == 0, "a fixture's Subscription is not the view's (rule 9)"
        ownsub = run("ownsub", [(a, "FAIL"), (b, "PASS"), (c, "PASS"), (d, "FAIL")],
                     [(a, "POST", f"{root}/subscriptions", []), (a, "GET", sub, [])])
        assert view_diff.diff(view, e1, ownsub, binds) == 1, "a Subscription made through the view owes nothing"
        # T-3945: after a suite fixture registered a Context Source at the root, an Entity write
        # and a Subscription through the view stay local (rules 7 and 9)
        json.dump({"id": "v", "operations": ["create_entity", "create_subscription"]}, open(view, "w"))
        writes = [(a, "POST", f"{root}/entities", []), (a, "POST", f"{root}/subscriptions", [])]
        registered = run("registered", [(a, "FAIL"), (b, "PASS"), (c, "PASS"), (d, "FAIL")], [
            (None, "POST", "http://h/ngsi-ld/v1/csourceRegistrations/", [], "TP/s.robot"), *writes])
        assert view_diff.diff(view, e1, registered, binds) == 0, "a write after a registration stays local"
        why = json.load(open(f"{registered}/owed-refusals.json"))["refusals"][0]["owed"]
        assert ["rule 7" in why[0], "rule 9" in why[1]] == [True, True], why
        unregistered = run("unregistered", [(a, "FAIL"), (b, "PASS"), (c, "PASS"), (d, "FAIL")], writes)
        assert view_diff.diff(view, e1, unregistered, binds) == 1, "without a registration a write owes nothing"
        json.dump({"v": {a: "T-0000"}}, open(known, "w"))
        assert view_diff.diff(view, e1, rootsub, binds) == 1, "a known TP that is owed now must fail"
        assert view_diff.diff(view, e1, f"{tmp}/nowhere", binds) == 1, "an empty run must fail"

        # T-3787: a view that hides types and Attributes owes the refusal of a read that names one,
        # in the query or in the body, under the names the suite's @context gives them
        restricted = f"{tmp}/restricted.json"
        json.dump({"id": "r", "operations": ["retrieve_entity", "query_entities", "query_entities_post"],
                   "selector": ["https://ngsi-ld-test-suite/context#Building"],
                   "rules": [{"types": ["https://ngsi-ld-test-suite/context#Building"],
                              "attributes": ["https://ngsi-ld-test-suite/context#name",
                                             "https://uri.etsi.org/ngsi-ld/location"]}]},
                  open(restricted, "w"))
        view_diff.KNOWN = f"{tmp}/none.json"
        hidden = run("hidden", [(a, "FAIL"), (b, "FAIL"), (c, "FAIL"), (d, "PASS")], [
            (a, "GET", f"{root}/entities?type=Vehicle", []),
            (b, "GET", f"{root}/entities?type=Building&q=airQualityLevel>2", []),
            (c, "POST", f"{root}/entityOperations/query", [],
             {"type": "Query", "entities": [{"type": "Building"}], "attrs": ["name", "airQualityLevel"]}),
            (d, "GET", f"{root}/entities?type=Building&attrs=name,location", []),
        ])
        assert view_diff.diff(restricted, e1, hidden, binds) == 0
        why = {r["tp"]: r["owed"] for r in json.load(open(f"{hidden}/owed-refusals.json"))["refusals"]}
        assert sorted(why) == [a, b, c], why
        assert "type Vehicle" in why[a][0] and "Attribute airQualityLevel" in why[b][0], why
        assert any("airQualityLevel" in w for w in why[c]), why
        shown = run("shown", [(a, "PASS"), (b, "PASS"), (c, "PASS"), (d, "FAIL")],
                    [(d, "GET", f"{root}/entities?type=Building&attrs=name,location", [])])
        assert view_diff.diff(restricted, e1, shown, binds) == 1, "a read of shown names owes nothing"
        # T-3982: a create the view refuses never makes the Entity, so a later TP naming it owes
        # the 404; a fixture that makes it again at the root clears that
        made = [
            (a, "POST", f"{root}/entities/", [], {"id": "urn:B1", "type": "Building"}),
            (a, "POST", f"{root}/entities", [], {"id": "urn:B2", "type": "Building"}),
            (None, "POST", "http://h/ngsi-ld/v1/entities", [], "TP/s.robot", {"id": "urn:B2", "type": "Building"}),
            (b, "GET", f"{root}/entities/urn%3AB1", []),
            (c, "GET", f"{root}/entities/urn:B2", []),
            (d, "GET", f"{root}/entities/urn:B3", []),
        ]
        unmade = run("unmade", [(a, "FAIL"), (b, "FAIL"), (c, "PASS"), (d, "PASS")], made)
        assert view_diff.diff(restricted, e1, unmade, binds) == 0
        why = {r["tp"]: r["owed"] for r in json.load(open(f"{unmade}/owed-refusals.json"))["refusals"]}
        assert sorted(why) == [a, b] and "never made" in why[b][0], why
        remade = run("remade", [(a, "FAIL"), (b, "FAIL"), (c, "FAIL"), (d, "PASS")], made)
        assert view_diff.diff(restricted, e1, remade, binds) == 1, "an Entity a fixture made again owes nothing"
        other = run("other", [(a, "FAIL"), (b, "FAIL"), (c, "PASS"), (d, "FAIL")], made)
        assert view_diff.diff(restricted, e1, other, binds) == 1, "an Entity nobody tried to make owes nothing"

        # T-3980: names in the path, the type of an entity id read, an orderBy, a listing, a join, a
        # query naming no type, and an answer lacking only hidden Attributes are owed by the view
        json.dump({"id": "r", "operations": ["retrieve_entity", "query_entities", "retrieve_attributes",
                                             "retrieve_attribute_info", "retrieve_entity_type_info",
                                             "temporal_retrieve"],
                   "selector": ["https://ngsi-ld-test-suite/context#Building"],
                   "rules": [{"types": ["https://ngsi-ld-test-suite/context#Building"],
                              "attributes": ["https://ngsi-ld-test-suite/context#name"]}]},
                  open(restricted, "w"))
        e, f, g, h, i, j, k, m = (f"TP/s.robot::01{n} T" for n in range(8))
        e1n = run("e1n", [(t, "PASS") for t in (e, f, g, h, i, j, k, m)])
        narrowed = run("narrowed", [(t, "FAIL") for t in (e, f, g, h, i, j, k, m)], [
            (e, "GET", f"{root}/attributes/airQualityLevel", []),
            (f, "GET", f"{root}/temporal/entities/urn:ngsi-ld:Vehicle:7", []),
            (g, "GET", f"{root}/entities?type=Building&orderBy=rankProbe", []),
            (h, "GET", f"{root}/attributes?details=false", []),
            (i, "GET", f"{root}/entities/urn:ngsi-ld:Building:1?join=inline&joinLevel=2", []),
            (j, "GET", f"{root}/entities?q=name==%22x%22", []),
            (k, "GET", f"{root}/entities/urn:ngsi-ld:Building:2", []),
            (m, "GET", f"{root}/entities/urn:ngsi-ld:Building:3", []),
        ])
        with open(f"{narrowed}/failures.csv", "w", newline="") as fh:
            w = csv.writer(fh)
            w.writerow(["suite", "test", "message"])
            w.writerow(["s", "011 T", "never read: a request names it"])
            w.writerow(["s", "016 T", "Item root['https://ngsi-ld-test-suite/context#airQualityLevel'] removed from dictionary."])
            w.writerow(["s", "017 T", "Item root['name'] removed from dictionary. KeyError: 'airQualityLevel'"])
        assert view_diff.diff(restricted, e1n, narrowed, binds) == 1, "an answer lacking a shown Attribute owes nothing"
        why = {r["tp"]: " ".join(r["owed"]) for r in json.load(open(f"{narrowed}/owed-refusals.json"))["refusals"]}
        assert sorted(why) == [e, f, g, h, i, j, k], why
        assert "Attribute airQualityLevel" in why[e] and "type Vehicle" in why[f], why
        assert "Attribute rankProbe" in why[g] and "lists only" in why[h] and "join" in why[i], why
        assert "naming no type" in why[j] and "lacks Attribute" in why[k], why
        assert view_diff.lacking("Item root['name'] removed from dictionary.", view_diff.hidden_by(
            json.load(open(restricted)), view_diff.core_terms())) == [], "a shown Attribute is not owed"
        assert view_diff.lacking("anything", None) == [], "a view that shows every name owes no lack"
        committed = json.load(open(f"{HERE}/../ci/views/etsi/etsi-restricted.json"))
        reads = {o["id"] for o in __import__("yaml").safe_load(open(view_diff.MATRIX))["operations"]
                 if o.get("kind") == "read"}
        assert set(committed["operations"]) == reads, "etsi-restricted offers the matrix's reads"
    print("  view diff ok (owed refusals pass, a deviation, a stale known entry or a missing TP fails;"
          " a hidden type or Attribute, or an Entity a refused create never made, owes its refusal,"
          " a narrowing view its narrowing)")


def check_view_residue():
    """T-3954: an Entity a TP created (setup or body) is deleted at the
    setup root, local only and in its Tenant, once the TP is over; a 404 is
    fine, and only creating requests count."""
    import http.server
    import threading
    import types

    # the listener's import; Robot itself is not needed by what runs here
    for name in ("robot", "robot.libraries", "robot.libraries.BuiltIn"):
        sys.modules.setdefault(name, types.ModuleType(name))
    sys.modules["robot.libraries.BuiltIn"].BuiltIn = object
    spec = importlib.util.spec_from_file_location("etsi_setup_url", f"{HERE}/etsi_setup_url.py")
    listener = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(listener)

    view = "http://h/x/views/all/ngsi-ld/v1"
    ids = listener.created_ids
    assert ids("POST", f"{view}/entities", {"id": "urn:a", "type": "T"}) == ["urn:a"]
    assert ids("POST", f"{view}/entities/", {"id": "urn:a"}) == ["urn:a"], "D018_03 posts to entities/"
    assert ids("POST", f"{view}/entityOperations/upsert?options=update",
               [{"id": "urn:a"}, {"id": "urn:b"}, {"type": "T"}, "x"]) == ["urn:a", "urn:b"]
    assert ids("POST", f"{view}/entityOperations/create", [{"id": "urn:c"}]) == ["urn:c"]
    assert ids("POST", f"{view}/entityOperations/delete", ["urn:a"]) == []
    assert ids("PATCH", f"{view}/entities", {"id": "urn:a"}) == []
    assert ids("POST", f"{view}/entities/urn:a/attrs", {"id": "urn:a"}) == []
    assert ids("POST", f"{view}/entities", None) == []

    seen = []

    class Root(http.server.BaseHTTPRequestHandler):
        def do_DELETE(self):
            seen.append((self.path, self.headers.get("NGSILD-Tenant")))
            self.send_response(404 if "gone" in self.path else 204)
            self.send_header("Content-Length", "0")
            self.end_headers()

        def log_message(self, *args):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Root)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        root = f"http://127.0.0.1:{server.server_port}/ngsi-ld/v1"
        tp = listener.etsi_setup_url(root)
        tp.created = [("urn:ngsi-ld:Vehicle:1", None), ("urn:ngsi-ld:Vehicle:1", None),
                      ("urn:gone", None), ("urn:t#1", "city")]
        tp.end_test(None, None)
        assert seen == [
            ("/ngsi-ld/v1/entities/urn%3Angsi-ld%3AVehicle%3A1?local=true", None),
            ("/ngsi-ld/v1/entities/urn%3Agone?local=true", None),
            ("/ngsi-ld/v1/entities/urn%3At%231?local=true", "city"),
        ], seen
        assert tp.created == [], "the list starts empty for the next TP"
        seen.clear()
        tp.end_test(None, None)
        assert seen == [], "a TP that created nothing deletes nothing"
    finally:
        server.shutdown()
    print("  view residue ok (created ids found, deleted once each at the root, local, in their Tenant)")


def check_recorder():
    """dev/etsi_setup_url.py records what a TP body sends, and nothing a failure left NOT RUN
    (T-4030: Robot still starts those steps, and their variables do not exist)."""
    with tempfile.TemporaryDirectory() as d:
        # a stand-in for RequestsLibrary: the listener reads the owner's name, never the library
        open(f"{d}/RequestsLibrary.py", "w").write("def post(url, data=None, headers=None, expected_status=None):\n    pass\n")
        open(f"{d}/t.robot", "w").write("""*** Settings ***
Library    RequestsLibrary
*** Variables ***
${url}    http://127.0.0.1:9/ngsi-ld/v1
${temporal_api_url}    http://127.0.0.1:9/ngsi-ld/v1/temporal
*** Test Cases ***
Runs
    &{headers}=    Create Dictionary    Content-Type=application/json
    POST    url=${url}/entities    data={"id": "urn:ngsi-ld:T:1"}    headers=${headers}
After A Failure
    Should Be Equal    1    2
    ${notify_url}=    Set Variable    ${url}/notify
    POST    url=${notify_url}    data={}
""")
        record = f"{d}/requests.jsonl"
        run = subprocess.run(
            [sys.executable, "-m", "robot", "--pythonpath", d, "--output", "NONE", "--report", "NONE",
             "--log", "NONE", "--listener", f"{HERE}/etsi_setup_url.py;http://127.0.0.1:9/ngsi-ld/v1;{record}",
             f"{d}/t.robot"], capture_output=True, text=True)
        assert "failed:" not in run.stdout + run.stderr, run.stdout + run.stderr
        lines = [json.loads(l) for l in open(record)]
        assert [(l["test"].split("::")[1], l["url"], l["headers"], l["body"]) for l in lines] == [
            ("Runs", "http://127.0.0.1:9/ngsi-ld/v1/entities", ["content-type"], {"id": "urn:ngsi-ld:T:1"})
        ], lines
    print("  recorder ok (a step that runs is recorded, a NOT RUN one is not and raises nothing)")


def check_local_context():
    """start/check/stop of etsi-local-context.sh on a tiny suite: the .pyc robot compiles from
    variables.py after `start` keeps the raw default and must not fail `check` (ci run
    40274454802: all three cells red with every TP passing), and `stop` restores every file."""
    import shutil
    import socket
    raw = "https://raw.githubusercontent.com/joinedcontext/Antares-NGSI-LD-Context-Broker/dev/ngsi-ld-test-suite/resources/jsonld-contexts/"
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = str(s.getsockname()[1])
    with tempfile.TemporaryDirectory() as d:
        os.makedirs(f"{d}/dev")
        shutil.copy(f"{HERE}/etsi-local-context.sh", f"{d}/dev/")
        suite = f"{d}/ngsi-ld-test-suite"
        for sub in ("resources/jsonld-contexts", "TP", "data"):
            os.makedirs(f"{suite}/{sub}")
        open(f"{suite}/resources/jsonld-contexts/ngsi-ld-test-suite-compound.jsonld", "w").write("{}")
        open(f"{suite}/resources/variables.py", "w").write(f"ngsild_test_suite_context = '{raw}x.jsonld'\n")
        data = f'{{"@context": "{raw}ngsi-ld-test-suite-compound.jsonld"}}'
        open(f"{suite}/data/e.jsonld", "w").write(data)
        env = dict(os.environ, CONTEXT_PORT=port, CONTEXT_STATE=f"{d}/state")
        sh = lambda *a: subprocess.run(["bash", f"{d}/dev/etsi-local-context.sh", *a], env=env,
                                       capture_output=True, text=True)
        local = sh("start").stdout.strip()
        try:
            assert local == f"http://localhost:{port}/ngsi-ld-test-suite-compound.jsonld", local
            assert raw not in open(f"{suite}/data/e.jsonld").read()
            subprocess.run([sys.executable, "-c", "import variables"], cwd=f"{suite}/resources", check=True)
            assert os.listdir(f"{suite}/resources/__pycache__"), "variables.py was not compiled"
            urllib.request.urlopen(local, timeout=5).read()
            chk = sh("check")
            assert chk.returncode == 0, chk.stdout + chk.stderr
        finally:
            sh("stop")
        assert open(f"{suite}/data/e.jsonld").read() == data, "stop did not restore the data file"
    print("  local context ok (a compiled variables.py does not fail check; stop restores)")


def check_ceiling_on_every_broker():
    """Every broker the ETSI stacks start, overlay replicas included, runs the 6.3.10 ceiling of
    9 the TTF 206 cases expect: the HA overlay's antares1b without it answered 200 on every read
    the LB sent it (roll-weekly run 40535598208, T-5858)."""
    import yaml
    compose = f"{HERE}/../compose-files"

    def load(name):
        with open(f"{compose}/{name}") as fh:
            return yaml.safe_load(fh)["services"]

    base = load("docker-compose-etsi.yml")
    for overlay in (None, "docker-compose-ha.yml", "docker-compose-roles.yml", "docker-compose-policy.yml"):
        services = {k: dict(v) for k, v in base.items()}
        for name, svc in (load(overlay) if overlay else {}).items():
            merged = dict(services.get(name, {}), **svc)
            merged["environment"] = dict(services.get(name, {}).get("environment") or {},
                                         **(svc.get("environment") or {}))
            services[name] = merged
        brokers = {k: v for k, v in services.items() if str(v.get("image", "")).startswith("antares")}
        assert brokers, overlay
        for name, svc in brokers.items():
            got = (svc.get("environment") or {}).get("ANTARES_TEMPORAL_INSTANCE_LIMIT")
            assert got == "9", f"{overlay or 'etsi'}: {name} runs ANTARES_TEMPORAL_INSTANCE_LIMIT={got!r}, not 9"
    print("  temporal ceiling ok (9 on every broker of the etsi, ha, roles and policy stacks)")


if __name__ == "__main__":
    print("etsi sampling self-check")
    check_proc_parsing()
    check_join()
    check_baseline()
    check_compare()
    check_view_diff()
    check_view_residue()
    check_recorder()
    check_local_context()
    check_ceiling_on_every_broker()
    print("OK")
