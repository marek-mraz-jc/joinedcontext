"""scripts/runner-rss-rig.py (T-5285): the arguments and image it reads, and its arithmetic."""

import importlib.util
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "runner_rss_rig", Path(__file__).resolve().parents[1] / "scripts" / "runner-rss-rig.py")
rig = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rig)


def test_set_args_reads_each_pair_and_unquotes():
    values = "args:\n  - -w\n  - --set\n  - a.b=true\n  # note\n  - --set\n  - 'm=root = if x { deleted() }'\n  - streams\n"
    assert rig.set_args(values) == ["a.b=true", "m=root = if x { deleted() }"]


def test_the_shipped_values_carry_the_mapping():
    sets = rig.set_args(rig.VALUES.read_text())
    assert "metrics.prometheus.use_histogram_timing=true" in sets
    assert any(s.startswith("metrics.mapping=") for s in sets), sets


def test_image_is_the_pinned_runner_digest():
    assert rig.image(rig.IMAGES.read_text()).startswith("ghcr.io/warpstreamlabs/bento@sha256:")


def test_rss_and_per_name():
    assert rig.rss_anon_kb("VmRSS:\t 1 kB\nRssAnon:\t   38740 kB\n") == 38740
    assert rig.per_name_mb(1024, 1024 + 512 * 1024, 1024) == 0.5


def test_the_stream_has_two_writes_among_its_steps():
    labels = [p["label"] for p in rig.stream(10)["pipeline"]["processors"]]
    assert len(labels) == 10 and labels[-2:] == ["write_0", "route_1_write_0"]


def test_heap_and_goroutines_read_the_pprof_text():
    heap = "heap profile: 1: 2 [3: 4] @ heap/1048576\n# HeapAlloc = 1\n# HeapInuse = 52428800\n# Stack = 3\n"
    assert rig.heap_inuse_mb(heap) == 50
    assert rig.goroutines("goroutine profile: total 249\n1120 @ 0x1\n") == 249


def test_per_run_is_growth_from_the_warm_look_to_the_last():
    seen = {0: {"rss_mb": 50.0}, 100: {"rss_mb": 55.0}, 500: {"rss_mb": 60.0}}
    assert rig.per_run_mb(seen) == 10.0 / 500


def test_the_runs_cover_the_seeds_input_kinds_and_end_on_their_own():
    kinds = rig.run_streams("http://stub")
    assert set(kinds) == {"generate_http", "http_client", "big", "zip"}
    for name, stream in kinds.items():
        source = stream["input"]
        assert source.get("generate", {}).get("count") == 1 or "read_until" in source, name
        writes = [p for p in stream["pipeline"]["processors"] if "http" in p and p["http"]["verb"] == "POST"]
        assert writes and writes[-1]["http"]["rate_limit"] == "rig__pipeline_egress", name
    unarchived = [p for p in kinds["zip"]["pipeline"]["processors"] for q in p.get("try", [p]) if "unarchive" in q]
    assert unarchived, kinds["zip"]


def test_the_stub_serves_the_feeds_the_token_and_the_writes():
    import json
    import threading
    import urllib.request
    from http.server import ThreadingHTTPServer

    server = ThreadingHTTPServer(("127.0.0.1", 0), rig.Stub)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"
    try:
        assert len(json.loads(urllib.request.urlopen(base + "/feed").read())) == 500
        assert urllib.request.urlopen(base + "/feed.zip").read()[:2] == b"PK"
        token = urllib.request.urlopen(urllib.request.Request(base + "/token", data=b"x", method="POST")).read()
        assert json.loads(token)["access_token"]
    finally:
        server.shutdown()
