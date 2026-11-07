#!/usr/bin/env python3
"""The regression gate over a TEAM Engine run of the OGC API - Features ETS (T-3747), and of the
SensorThings ETS ets-sta10 against ci/sta-compliance-baseline.json (T-4878), and of the OGC API -
Tiles ETS ets-ogcapi-tiles10 against ci/ogc-tiles-compliance-baseline.json (T-5195).

    dev/ogc-compliance/baseline.py write RESULTS.xml BASELINE.json   record a run as the baseline
    dev/ogc-compliance/baseline.py check RESULTS.xml BASELINE.json   red when a baseline pass is lost
    dev/ogc-compliance/baseline.py --self-test

TEAM Engine answers a run with a TestNG document: one <test> per conformance class, one
<test-method> per check, its status PASS, FAIL or SKIP (configuration methods do not count).
A test is `Core :: LandingPage.landingPageValidation`, `#2`, `#3` added when a data provider
runs the method again. `check` fails on any test that passed in the baseline and does not pass
now (failed, skipped or gone), never on a known failure; a new pass is listed so the baseline
can be raised. Both print the per-class table of the run as Markdown for the job summary.
"""

import json
import sys
import xml.etree.ElementTree as ET


def parse(xml):
    """`({test: status}, {class: [passed, failed, skipped]})` of one TestNG document."""
    results, classes = {}, {}
    for test in ET.fromstring(xml).iter("test"):
        group = test.get("name", "")
        for cls in test.iter("class"):
            short = cls.get("name", "").rsplit(".", 1)[-1]
            for method in cls.iter("test-method"):
                if method.get("is-config") == "true":
                    continue
                status = method.get("status", "")
                base = key = f"{group} :: {short}.{method.get('name', '')}"
                n = 2
                while key in results:
                    key, n = f"{base} #{n}", n + 1
                results[key] = status
                tally = classes.setdefault(group, [0, 0, 0])
                if status in ("PASS", "FAIL", "SKIP"):
                    tally[("PASS", "FAIL", "SKIP").index(status)] += 1
    return results, classes


def table(classes):
    rows = ["| Conformance class | Passed | Failed | Skipped | Pass rate (of run) |", "|---|---:|---:|---:|---:|"]
    for name, (p, x, s) in classes.items():
        rate = f"{100 * p / (p + x):.1f} %" if p + x else "-"
        rows.append(f"| {name} | {p} | {x} | {s} | {rate} |")
    return "\n".join(rows)


def counts(results):
    return {s: sum(1 for v in results.values() if v == s) for s in ("PASS", "FAIL", "SKIP")}


def lost(baseline, results):
    """Tests the baseline passes and the run does not, sorted."""
    return sorted(k for k, v in baseline.items() if v == "PASS" and results.get(k) != "PASS")


def gained(baseline, results):
    return sorted(k for k, v in results.items() if v == "PASS" and baseline.get(k) != "PASS")


def main(argv):
    if argv == ["--self-test"]:
        return self_test()
    if len(argv) != 3 or argv[0] not in ("write", "check"):
        sys.exit(__doc__)
    mode, run, path = argv
    with open(run, encoding="utf-8") as f:
        try:
            results, classes = parse(f.read())
        except ET.ParseError as e:
            sys.exit(f"{run}: no TestNG document ({e}): did TEAM Engine answer with an error page?")
    if not results:
        sys.exit(f"{run}: no test result: the ETS ran no test method")
    print(table(classes))
    if mode == "write":
        with open(path, "w", encoding="utf-8") as f:
            json.dump({"counts": counts(results), "results": results}, f, indent=1, sort_keys=True)
            f.write("\n")
        print(f"\nbaseline written: {counts(results)}")
        return 0
    with open(path, encoding="utf-8") as f:
        baseline = json.load(f)["results"]
    down, up = lost(baseline, results), gained(baseline, results)
    print(f"\nrun {counts(results)}, baseline {counts(baseline)}")
    if up:
        print(f"\n{len(up)} new pass(es), raise the baseline:\n" + "\n".join(f"- {k}" for k in up))
    if down:
        print(f"\n{len(down)} baseline pass(es) lost:\n" + "\n".join(f"- {k}" for k in down))
        return 1
    return 0


SAMPLE = """<testng-results><suite name="ogcapi-features-1.0">
  <test name="Core">
    <class name="org.opengis.cite.ogcapifeatures10.conformance.core.general.LandingPage">
      <test-method status="PASS" name="initCommonFixture" is-config="true"/>
      <test-method status="PASS" name="landingPageValidation"/>
      <test-method status="FAIL" name="apiDefinition"/>
    </class>
    <class name="org.opengis.cite.ogcapifeatures10.conformance.core.collections.FeaturesBBox">
      <test-method status="PASS" name="validateFeaturesWithBboxOperation"/>
      <test-method status="PASS" name="validateFeaturesWithBboxOperation"/>
    </class>
  </test>
  <test name="GeoJSON">
    <class name="org.opengis.cite.ogcapifeatures10.conformance.geojson.Items">
      <test-method status="SKIP" name="geojson"/>
    </class>
  </test>
</suite></testng-results>
"""


def self_test():
    results, classes = parse(SAMPLE)
    assert results == {
        "Core :: LandingPage.landingPageValidation": "PASS",
        "Core :: LandingPage.apiDefinition": "FAIL",
        "Core :: FeaturesBBox.validateFeaturesWithBboxOperation": "PASS",
        "Core :: FeaturesBBox.validateFeaturesWithBboxOperation #2": "PASS",
        "GeoJSON :: Items.geojson": "SKIP",
    }, results
    assert classes == {"Core": [3, 1, 0], "GeoJSON": [0, 0, 1]}, classes
    assert "| Core | 3 | 1 | 0 | 75.0 % |" in table(classes)
    assert "| GeoJSON | 0 | 0 | 1 | - |" in table(classes)
    assert parse("<testng-results><suite name='x'/></testng-results>") == ({}, {})
    now = dict(results)
    now["Core :: FeaturesBBox.validateFeaturesWithBboxOperation #2"] = "SKIP"
    del now["Core :: LandingPage.landingPageValidation"]
    now["Core :: LandingPage.apiDefinition"] = "PASS"
    assert lost(results, now) == [
        "Core :: FeaturesBBox.validateFeaturesWithBboxOperation #2",
        "Core :: LandingPage.landingPageValidation",
    ]
    assert gained(results, now) == ["Core :: LandingPage.apiDefinition"]
    assert lost(results, results) == []
    print("self-test: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
