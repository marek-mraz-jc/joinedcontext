import importlib.util
import xml.etree.ElementTree as ET
from pathlib import Path

import pytest


@pytest.fixture(scope="module")
def converter():
    script = Path(__file__).resolve().parent.parent / "ci" / "policies" / "report_to_junit.py"
    spec = importlib.util.spec_from_file_location("report_to_junit", script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def entry(policy, rule, result, name="app", kind="Deployment", message="msg", severity="medium"):
    return {
        "policy": policy,
        "rule": rule,
        "result": result,
        "message": message,
        "severity": severity,
        "category": "Best Practices",
        "resources": [{"kind": kind, "name": name}],
    }


def to_xml(converter, results):
    policies = converter.parse_report({"results": results})
    return converter.build_junit(policies)


def test_fail_becomes_failure_element(converter):
    root = to_xml(converter, [entry("p1", "r1", "fail", message="boom")])
    assert root.attrib["tests"] == "1"
    assert root.attrib["failures"] == "1"
    failure = root.find("./testsuite/testcase/failure")
    assert failure is not None
    assert failure.attrib["message"] == "boom"
    assert failure.attrib["type"] == "medium"


def test_warn_becomes_error_element(converter):
    root = to_xml(converter, [entry("p1", "r1", "warn")])
    assert root.attrib["failures"] == "0"
    assert root.attrib["errors"] == "1"
    assert root.find("./testsuite/testcase/error") is not None


def test_pass_is_empty_testcase(converter):
    root = to_xml(converter, [entry("p1", "r1", "pass")])
    testcase = root.find("./testsuite/testcase")
    assert testcase is not None
    assert len(testcase) == 0
    assert root.attrib["failures"] == "0"
    assert root.attrib["errors"] == "0"


def test_autogen_rule_merges_with_base_rule(converter):
    root = to_xml(converter, [
        entry("p1", "check", "pass"),
        entry("p1", "autogen-check", "fail"),
    ])
    # one merged testcase, worst status (fail) wins
    testcases = root.findall("./testsuite/testcase")
    assert len(testcases) == 1
    assert testcases[0].attrib["classname"] == "p1.check"
    assert testcases[0].find("failure") is not None


def test_messages_of_merged_variants_are_concatenated(converter):
    root = to_xml(converter, [
        entry("p1", "check", "fail", message="first"),
        entry("p1", "autogen-check", "fail", message="second"),
    ])
    failure = root.find("./testsuite/testcase/failure")
    assert failure.text == "first\nsecond"


def test_one_testsuite_per_policy(converter):
    root = to_xml(converter, [
        entry("p1", "r1", "pass"),
        entry("p2", "r1", "fail"),
    ])
    suites = {ts.attrib["name"]: ts for ts in root.findall("testsuite")}
    assert set(suites) == {"p1", "p2"}
    assert suites["p2"].attrib["failures"] == "1"


def test_testcase_name_contains_resource_and_kind(converter):
    root = to_xml(converter, [entry("p1", "r1", "pass", name="web", kind="StatefulSet")])
    testcase = root.find("./testsuite/testcase")
    assert testcase.attrib["name"] == "web (StatefulSet)"


def test_entries_without_resources_are_skipped(converter):
    results = [entry("p1", "r1", "fail")]
    results[0]["resources"] = []
    root = to_xml(converter, results)
    assert root.attrib["tests"] == "0"


def test_output_is_valid_xml_roundtrip(converter):
    root = to_xml(converter, [
        entry("p1", "r1", "fail", message='with "quotes" & <angles>'),
    ])
    parsed = ET.fromstring(ET.tostring(root, encoding="unicode"))
    assert parsed.find("./testsuite/testcase/failure") is not None
