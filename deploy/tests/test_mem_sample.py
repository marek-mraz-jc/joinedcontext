"""scripts/mem-sample.py (T-5282): parsing, workload names, p95 and the suggested request."""

import importlib.util
from pathlib import Path

import pytest

spec = importlib.util.spec_from_file_location(
    "mem_sample", Path(__file__).resolve().parents[1] / "scripts" / "mem-sample.py")
ms = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ms)


def test_workload_strips_deployment_and_statefulset_suffixes():
    assert ms.workload("portal-7d9c8f6b5f-x2k4p") == "portal"
    assert ms.workload("postgres-cluster-1") == "postgres-cluster"
    assert ms.workload("ckan-solr-0") == "ckan-solr"
    assert ms.workload("node-exporter-x2k4p") == "node-exporter"
    assert ms.workload("keycloak") == "keycloak"


@pytest.mark.parametrize("q,expected", [("512Mi", 512), ("2Gi", 2048), ("1024Ki", 1), ("1048576", 1)])
def test_mi(q, expected):
    assert ms.mi(q) == expected


def test_mi_refuses_garbage():
    with pytest.raises(ValueError):
        ms.mi("12 bananas")


def test_parse_top_skips_malformed_lines():
    text = "jc portal-7d9c8f6b5f-x2k4p portal 12m 474Mi\nerror: metrics not available\n"
    assert list(ms.parse_top(text)) == [("jc", "portal-7d9c8f6b5f-x2k4p", "portal", 474)]


def test_report_p95_and_suggestion_against_request():
    rows = [("t", "jc", f"portal-7d9c8f6b5f-{s}", "portal", m)
            for s, m in zip(["zzzzz"] * 19 + ["bbbbb"], list(range(400, 419)) + [900])]
    pods = {"items": [{"metadata": {"namespace": "jc", "name": "portal-7d9c8f6b5f-zzzzz"},
                       "spec": {"containers": [{"name": "portal",
                                                "resources": {"requests": {"memory": "320Mi"}}}]}}]}
    [(ns, wl, c, n, p, mx, req, sug)] = ms.report(rows, ms.requests_of(pods))
    assert (ns, wl, c, n, p, mx, req) == ("jc", "portal", "portal", 20, 418, 900, 320)
    assert sug == 512  # 418 * 1.2 = 501.6 -> next 16 Mi step


def test_report_floor_and_missing_request():
    [row] = ms.report([("t", "ns", "tiny-0", "c", "3")], {})
    assert row[-2] is None and row[-1] == 32


def test_by_age_reads_rss_at_each_age_and_skips_pods_without_a_start():
    mib = 2**20
    rss = {"data": {"result": [
        {"metric": {"pod": "portal-b", "image": "ghcr.io/x@sha256:aad2d85afa0805cbd145"},
         "values": [[1000 + 900, str(100 * mib)], [1000 + 3600, str(200 * mib)], [1000 + 7300, str(250 * mib)]]},
        {"metric": {"pod": "portal-a", "image": "ghcr.io/x@sha256:dfbbb3df4532"},
         "values": [[500 + 18000, str(417 * mib)]]},
        {"metric": {"pod": "portal-gone"}, "values": [[1, str(mib)]]},
    ]}}
    starts = {"data": {"result": [{"metric": {"pod": "portal-b"}, "value": [0, "1000"]},
                                  {"metric": {"pod": "portal-a"}, "value": [0, "500"]}]}}
    rows = ms.by_age(rss, starts, ages=(0.25, 1, 2, 5))
    assert [r[0] for r in rows] == ["portal-a", "portal-b"]
    assert rows[0][3] == [None, None, None, 417]
    assert rows[1][1] == "aad2d85afa08"
    assert rows[1][3] == [100, 200, 250, None]
    assert rows[1][4] == 250
