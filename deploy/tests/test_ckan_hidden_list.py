"""EP-145, T-4846: the catalogue's publish-gate hidden list is generated, never typed."""
import importlib.util
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("ckan_hidden_list", ROOT / "scripts/ckan-hidden-list.py")
gen = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gen)

GATE = {"at": "2026-10-17T21:34Z", "rows": [
    {"package": "bb-zz", "verdict": "hide", "checks": {
        "ngsi-ld": {"state": "pass", "reason": ""},
        "accurate": {"state": "fail", "reason": "no entity behind https://dev.joinedcontext.com/api/endpoint/x/"},
        "queryable": {"state": "fail", "reason": "later check"}}},
    {"package": "bb-aa", "verdict": "hide", "checks": {"ngsi-ld": {"state": "fail", "reason": "\"403\" TooManyResults"}}},
    {"package": "bb-kept", "verdict": "keep", "checks": {"ngsi-ld": {"state": "pass", "reason": ""}}},
    {"package": "bb-back", "verdict": "hide", "checks": {"ngsi-ld": {"state": "fail", "reason": "old"}}},
]}


def test_the_committed_list_matches_its_digest():
    _, block, _ = gen.split(gen.VALUES.read_text(encoding="utf-8"))
    assert gen.verify(block) == []
    kept = gen.passed_back(gen.PASSED_BACK.read_text(encoding="utf-8"))
    assert not set(kept) & {gen.ENTRY.match(line).group(1) for line in block.splitlines()[3:-1]}, \
        "a package passed back in is not hidden"


def test_each_hidden_row_becomes_its_first_failed_check_sorted_and_a_passed_back_one_stays_out():
    block = gen.render(GATE, {"bb-back": "T-1 rows came back"}, "dev.joinedcontext.com")
    lines = block.splitlines()
    assert lines[0] == gen.BEGIN and lines[-1] == gen.END and lines[2] == "hidden:"
    assert lines[3:-1] == [
        '  bb-aa: "ngsi-ld: \\"403\\" TooManyResults"',
        '  bb-zz: "accurate: no entity behind https://api.{{ .Values.global.domain }}/api/endpoint/x/"',
    ]
    assert gen.verify(block) == []


@pytest.mark.parametrize("edit", [
    lambda b: b.replace('  bb-aa: "ngsi-ld: \\"403\\" TooManyResults"\n', ""),
    lambda b: b.replace("hidden:\n", "hidden:\n  bb-typed: \"by hand\"\n"),
    lambda b: b.replace("TooManyResults", "Too many"),
])
def test_a_hand_edit_fails_the_check(edit):
    block = gen.render(GATE, {}, "dev.joinedcontext.com")
    assert gen.verify(edit(block)), "a hand edit must not pass"


def test_a_hidden_row_without_a_failed_check_and_a_bad_passed_back_line_are_refused():
    with pytest.raises(ValueError, match="without a failed check"):
        gen.reasons({"rows": [{"package": "x", "verdict": "hide", "checks": {}}]}, None)
    with pytest.raises(ValueError, match="not `name: why`"):
        gen.passed_back("bb-x:\n")
