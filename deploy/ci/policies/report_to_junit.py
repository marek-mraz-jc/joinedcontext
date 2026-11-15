#!/usr/bin/env python3
"""
Convert a kyverno ClusterReport (openreports.io/v1alpha1) to JUnit XML.

Structure:
  <testsuites>
    <testsuite name="{policy}">                          -- one per policy
      <testcase classname="{policy}.{rule}"
                name="{name} ({kind})">                  -- one per (rule, resource)
        <failure>  on fail
        <error>    on warn
        (empty)    on pass
      </testcase>
    </testsuite>
  </testsuites>

autogen-* rule variants are merged under their base rule name.
When multiple variants produce different messages, they are concatenated.
Status priority: fail > warn > pass.
"""

import sys
import xml.etree.ElementTree as ET
from collections import defaultdict
import yaml

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------

STATUS_PRIORITY = {"fail": 3, "warn": 2, "skip": 1, "pass": 0}


def base_rule(rule: str) -> str:
    """Strip autogen- prefix so variants merge with the canonical rule."""
    return rule.removeprefix("autogen-")


def resource_id(resource: dict) -> str:
    """Human-readable resource identifier used as testcase name."""
    kind = resource.get("kind", "")
    name = resource.get("name", "")
    return f"{name} ({kind})"


def merge_status(existing: str, incoming: str) -> str:
    """Return the higher-priority status."""
    if STATUS_PRIORITY.get(incoming, 0) > STATUS_PRIORITY.get(existing, 0):
        return incoming
    return existing


def merge_message(existing: str, incoming: str) -> str:
    """Concatenate messages when both are non-empty and differ."""
    if not existing:
        return incoming
    if not incoming or incoming == existing:
        return existing
    return f"{existing}\n{incoming}"


# ---------------------------------------------------------------------------
# parsing
# ---------------------------------------------------------------------------

def parse_report(data: dict) -> dict:
    """
    Returns:
      {
        policy: {
          (rule, resource_id): {"status": str, "message": str,
                                "severity": str, "category": str}
        }
      }
    """
    results = data.get("results") or []

    # policies[policy][(rule, res_id)] = {status, message, severity, category}
    policies: dict = defaultdict(dict)

    for entry in results:
        policy = entry.get("policy", "unknown-policy")
        rule = base_rule(entry.get("rule", "unknown-rule"))
        status = entry.get("result", "pass")
        message = entry.get("message", "")
        severity = entry.get("severity", "")
        category = entry.get("category", "")

        resources = entry.get("resources") or []
        if not resources:
            continue  # nothing actionable without a concrete resource

        for resource in resources:
            res_id = resource_id(resource)
            key = (rule, res_id)

            existing = policies[policy].get(key)
            if existing is None:
                policies[policy][key] = {
                    "status": status,
                    "message": message,
                    "severity": severity,
                    "category": category,
                }
            else:
                # merge: worst status wins, concatenate messages
                policies[policy][key]["status"] = merge_status(existing["status"], status)
                policies[policy][key]["message"] = merge_message(existing["message"], message)

    return policies


# ---------------------------------------------------------------------------
# XML generation
# ---------------------------------------------------------------------------

def build_junit(policies: dict) -> ET.Element:
    total_tests = sum(len(cases) for cases in policies.values())
    total_failures = sum(
        sum(1 for c in cases.values() if c["status"] == "fail")
        for cases in policies.values()
    )
    total_errors = sum(
        sum(1 for c in cases.values() if c["status"] == "warn")
        for cases in policies.values()
    )
    total_skipped = sum(
        sum(1 for c in cases.values() if c["status"] == "skip")
        for cases in policies.values()
    )

    testsuites = ET.Element("testsuites", attrib={
        "name": "Kyverno Policy Scan",
        "tests": str(total_tests),
        "failures": str(total_failures),
        "errors": str(total_errors),
        "skipped": str(total_skipped),
        "time": "0",
    })

    for policy, cases in sorted(policies.items()):
        failures = sum(1 for c in cases.values() if c["status"] == "fail")
        errors = sum(1 for c in cases.values() if c["status"] == "warn")
        skipped = sum(1 for c in cases.values() if c["status"] == "skip")

        testsuite = ET.SubElement(testsuites, "testsuite", attrib={
            "name": policy,
            "tests": str(len(cases)),
            "failures": str(failures),
            "errors": str(errors),
            "skipped": str(skipped),
            "time": "0",
        })

        for (rule, res_id), info in sorted(cases.items()):
            testcase = ET.SubElement(testsuite, "testcase", attrib={
                "classname": f"{policy}.{rule}",
                "name": res_id,
                "time": "0",
            })

            status = info["status"]
            message = info["message"]
            severity = info["severity"]

            if status == "fail":
                failure = ET.SubElement(testcase, "failure", attrib={
                    "message": message,
                    "type": severity,
                })
                failure.text = message
            elif status == "warn":
                error = ET.SubElement(testcase, "error", attrib={
                    "message": message,
                    "type": severity,
                })
                error.text = message
            elif status == "skip":
                ET.SubElement(
                    testcase, "skipped",
                    attrib={"message": message} if message else {},
                )
            # pass → empty testcase element

    return testsuites


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def main() -> None:
    if len(sys.argv) > 1:
        with open(sys.argv[1]) as fh:
            raw = fh.read()
    elif not sys.stdin.isatty():
        raw = sys.stdin.read()
    else:
        sys.exit("Usage: report_to_junit.py [<report.yaml>]  (or pipe to stdin)")

    data = yaml.safe_load(raw)
    if not isinstance(data, dict):
        sys.exit("Error: expected a YAML mapping (ClusterReport CR)")

    policies = parse_report(data)
    root = build_junit(policies)

    tree = ET.ElementTree(root)
    ET.indent(tree, space="  ")
    sys.stdout.write('<?xml version="1.0" encoding="UTF-8"?>\n')
    sys.stdout.write(ET.tostring(root, encoding="unicode"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
