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


class FakeHttp:
    """Answers each POST from `answers` in turn and keeps what was sent."""

    def __init__(self, *answers):
        self.answers, self.sent = list(answers), []

    def call(self, method, path, token=None, headers=None, body=None):
        self.sent.append(body)
        result = self.answers.pop(0)
        return 200, {}, json.dumps({"jsonrpc": "2.0", "id": body.get("id"), "result": result}).encode()


D10 = {"isError": True, "content": [{"type": "text", "text": "the answer is larger than this endpoint returns at "
                                     "once (65536 bytes); narrow it with pick or attrs"}]}


def test_an_answer_over_the_budget_is_asked_again_as_its_hint_says():
    page = {"isError": False, "structuredContent": {"entities": [{"id": "urn:ngsi-ld:F:a", "type": "F"}]}}
    http = FakeHttp(D10, page)
    a, note = client.Mcp(http, "/api/endpoint/x/mcp").query({"type": ["F"], "limit": 5})
    assert a.ok and client.entities_of(a.data) == [{"id": "urn:ngsi-ld:F:a", "type": "F"}] and note
    assert http.sent[1]["params"]["arguments"] == {"type": ["F"], "limit": 5, "pick": ["id", "type"]}
    # One that still does not fit stays the refusal; one already narrowed is not asked again.
    a, _ = client.Mcp(FakeHttp(D10, D10), "/api/mcp").query({"type": ["F"]})
    assert a.refused
    http = FakeHttp(D10)
    a, note = client.Mcp(http, "/api/mcp").query({"type": ["F"], "pick": ["id"]})
    assert a.refused and not note and len(http.sent) == 1


def test_hub_calls_from_every_thread_keep_under_the_hub_refill_rate():
    import threading
    import time
    pacer = client.Pacer(0.02)
    stamps, lock = [], threading.Lock()

    def worker():
        for _ in range(5):
            pacer.wait()
            with lock:
                stamps.append(time.monotonic())

    threads = [threading.Thread(target=worker) for _ in range(6)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    stamps.sort()
    # 30 calls take at least 29 intervals, whichever thread makes them.
    assert len(stamps) == 30 and stamps[-1] - stamps[0] >= 29 * 0.02 * 0.95
    # 600 a minute is 0.1 s a call: the hub's own pace stays under it, other doors are not paced.
    assert client.HUB_INTERVAL > 0.1
    assert client.Mcp(None, "/api/mcp").pacer is client.HUB_PACER
    assert client.Mcp(None, "/api/endpoint/x/mcp").pacer is None


def _endpoint(grants):
    return suite.ex.Endpoint(project="p", name="e", space="s", tenant="t", slug="slug", audience="public",
                             representations=["mcp"], grants=grants)


def _expect(tenants, temporal=None):
    x = suite.Expect.__new__(suite.Expect)
    x.index = {"tenants": {"t": tenants}, "temporal": {"t": temporal or {}}}
    return x


PUBLIC = {"kind": "role", "id": "public"}


def test_the_by_id_sample_is_a_row_the_grant_opens():
    slot = {"count": 3, "sample": "urn:ngsi-ld:K:fmi-1", "psample": {"^urn:ngsi-ld:K:sw-": "urn:ngsi-ld:K:sw-1"}}
    x = _expect({"K": slot})
    patterned = suite.ex.Grant("g", PUBLIC, {"retrieveOps"}, [suite.ex.Selector("K", "^urn:ngsi-ld:K:sw-")])
    assert x.sample(_endpoint([patterned]), "K") == "urn:ngsi-ld:K:sw-1"
    # A scopeQ or q grant: the export cannot say which row passes it, the door's own page says.
    scoped = suite.ex.Grant("g", PUBLIC, {"retrieveOps"}, [suite.ex.Selector("K", None)], "scopeQ")
    assert x.sample(_endpoint([scoped]), "K") is None
    whole = suite.ex.Grant("g", PUBLIC, {"retrieveOps"}, [suite.ex.Selector("K", None)])
    assert x.sample(_endpoint([scoped, whole]), "K") == "urn:ngsi-ld:K:fmi-1"
    # An expiring type's exported row may be gone (an Alert past its expiresAt).
    slot["expires"] = True
    assert x.sample(_endpoint([whole]), "K") is None


def test_the_history_sample_is_a_series_the_temporal_grant_opens():
    series = {"id": "urn:ngsi-ld:A:fmi-1", "attr": "aqi", "newest": "2026-11-08T17:00:00Z",
              "psample": {"^urn:ngsi-ld:A:sw-": {"id": "urn:ngsi-ld:A:sw-1", "attr": "no2",
                                                 "newest": "2026-11-08T16:00:00Z"}}}
    x = _expect({"A": {"count": 2, "sample": "urn:ngsi-ld:A:fmi-1", "patterns": {"^urn:ngsi-ld:A:sw-": 1}}},
                {"A": series})
    g = suite.ex.Grant("g", PUBLIC, {"queryTemporal"}, [suite.ex.Selector("A", "^urn:ngsi-ld:A:sw-")])
    typ, s = x.temporal(_endpoint([g]), ["A"])
    assert (typ, s["id"], s["attr"]) == ("A", "urn:ngsi-ld:A:sw-1", "no2")


def test_retrieve_ops_alone_grants_no_history():
    # CIM 009 Table 4.20-2: retrieveOps is retrieveEntity and queryEntity.
    read = suite.ex.Grant("g", PUBLIC, {"retrieveOps", "createSubscription"}, [suite.ex.Selector("O", None)])
    assert not read.temporal() and read.reads() and not read.writes()
    x = _expect({"O": {"count": 1, "sample": "urn:ngsi-ld:O:1"}},
                {"O": {"id": "urn:ngsi-ld:O:1", "attr": "n", "newest": "2026-11-08T18:00:00Z"}})
    assert x.temporal(_endpoint([read]), ["O"]) is None


def test_a_narrowed_count_past_the_broker_deadline_is_a_skip_and_any_other_miss_fails():
    late = answer({"jsonrpc": "2.0", "id": 1, "result": {"isError": True, "content": [{"type": "text", "text":
                   "the broker did not answer within ten seconds; narrow the window or the types and ask again"}]}})
    counted = answer({"jsonrpc": "2.0", "id": 1, "result": {"structuredContent": {"entities": [], "total": 4}}})
    lost = answer({"jsonrpc": "2.0", "id": 1, "result": {"structuredContent": {"entities": []}}})
    assert suite.narrowed_count(counted)[0] == "pass"
    assert suite.narrowed_count(late)[0] == "skip"
    # A count that answers without a total, or a door that breaks, still fails.
    assert suite.narrowed_count(lost)[0] == "fail"
    assert suite.narrowed_count(answer(b"boom", status=500))[0] == "fail"
