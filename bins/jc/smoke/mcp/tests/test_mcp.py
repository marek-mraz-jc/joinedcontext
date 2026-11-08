"""Unit tests of the mcp suite's reading of MCP answers (the live proof is a rehearsal run).

    python3 -m pytest bins/jc/smoke/mcp/tests -q
"""

import importlib.util
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent.parent))
sys.path.append(str(HERE.parent.parent / "endpoints"))

import client  # noqa: E402

# Loaded under its own name: the endpoints suite is also a `suite` module.
_spec = importlib.util.spec_from_file_location("smoke_mcp_suite", HERE.parent / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)


def answer(body, status=200, headers=None):
    return client.Answer(status, headers or {}, json.dumps(body).encode() if not isinstance(body, bytes) else body)


def test_an_sse_body_is_read_from_its_last_data_event():
    raw = b'event: message\ndata: {"jsonrpc":"2.0","id":1,"result":{"ok":1}}\n\n'
    assert client.parse_body(raw) == {"jsonrpc": "2.0", "id": 1, "result": {"ok": 1}}


def test_structured_content_wins_and_a_fenced_text_is_read_otherwise():
    assert client.payload({"structuredContent": {"entities": []}, "content": []}) == {"entities": []}
    fenced = {"content": [{"type": "text", "text": "untrusted data:\n```\n{\"entities\": [{\"id\": \"x\"}]}\n```"}]}
    assert client.entities_of(client.payload(fenced)) == [{"id": "x"}]


def test_a_tool_error_a_jsonrpc_error_and_a_401_are_refusals_and_a_result_is_not():
    assert answer({"jsonrpc": "2.0", "id": 1, "result": {"isError": True, "content": []}}).refused
    assert answer({"jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": "no"}}).refused
    assert answer(b"", status=401).refused
    ok = answer({"jsonrpc": "2.0", "id": 1, "result": {"isError": False, "structuredContent": {"entities": []}}})
    assert ok.ok and not ok.refused
    # A 500 is neither: a broken door never passes a refusal check.
    assert not answer(b"boom", status=500).refused


def test_temporal_instances_are_counted_in_every_representation():
    normalized = {"entities": [{"id": "a", "type": "T", "pm10": [{"type": "Property", "value": 1},
                                                              {"type": "Property", "value": 2}]}]}
    simplified = {"entities": [{"id": "a", "type": "T", "pm10": {"type": "Property", "values": [[1, "t"], [2, "u"]]}}]}
    single = {"entities": [{"id": "a", "type": "T", "pm10": {"type": "Property", "value": 1}}]}
    empty = {"entities": [{"id": "a", "type": "T"}]}
    assert [suite.instances(x) for x in (normalized, simplified, single, empty)] == [2, 2, 1, 0]


def test_totals_are_found_at_the_top_or_one_level_down():
    assert client.total_of({"entities": [], "total": 7}) == 7
    assert client.total_of({"result": {"total": 3}}) == 3
    assert client.total_of({"entities": []}) is None
