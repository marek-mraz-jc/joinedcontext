"""notify_once.py's verdict: each written value once, a missing or doubled one named."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import notify_once


def note(*values):
    return {"data": [{"id": "urn:ngsi-ld:NotifyOnce:x", "seq": {"type": "Property", "value": v}} for v in values]}


def test_each_value_once_passes_and_the_initial_state_is_not_counted():
    assert notify_once.tally([note(-1), note(0), note(1), note(2)], 3) == {"missing": [], "doubled": []}


def test_a_doubled_and_a_missing_write_are_named():
    assert notify_once.tally([note(0), note(0), note(2)], 3) == {"missing": [1], "doubled": [0]}


def test_a_base_off_loopback_is_refused():
    import pytest
    with pytest.raises(SystemExit):
        notify_once.loopback("http://jc.example:8080")
