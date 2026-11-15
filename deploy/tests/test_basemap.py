"""T-4638: the background map from the cluster. The rendered dev environment carries the
CronJob, the volume, the server and its route on tiles.{domain}; the style asks no other host;
the server is read-only; the refresh checks the room before it writes and swaps by rename."""

import json
import re
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
COMPONENT = ROOT / "components/basemap"
STYLES = COMPONENT / "charts/tiles/files/styles"
HOST = "tiles.dev.joinedcontext.com"


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"expected one {kind}/{name}, got {len(found)}"
    return found[0]


def test_dev_renders_the_job_the_volume_the_server_and_its_route(dev):
    cron = one(dev, "CronJob", "basemap-tiles")["spec"]
    assert cron["schedule"] == "0 3 1 * *" and cron["timeZone"] == "UTC"
    assert cron["concurrencyPolicy"] == "Forbid"
    pvc = one(dev, "PersistentVolumeClaim", "basemap-tiles")
    assert pvc["metadata"]["annotations"]["helm.sh/resource-policy"] == "keep"
    server = one(dev, "Deployment", "basemap-tiles")["spec"]["template"]["spec"]
    assert server["automountServiceAccountToken"] is False
    data = next(v for v in server["volumes"] if v["name"] == "data")
    assert data["persistentVolumeClaim"]["readOnly"] is True
    mount = next(m for m in server["containers"][0]["volumeMounts"] if m["name"] == "data")
    assert mount["readOnly"] is True, "the server never writes the tiles"
    job = cron["jobTemplate"]["spec"]["template"]["spec"]
    # One volume, two pods: on an SELinux node both need the same MCS level or the server cannot
    # read what the job wrote (403 on dev 2026-10-18); the type stays the runtime's container_t.
    levels = {s["securityContext"]["seLinuxOptions"]["level"] for s in (server, job)}
    assert len(levels) == 1 and all(set(s["securityContext"]["seLinuxOptions"]) == {"level"} for s in (server, job))
    for spec in (server, job):
        assert spec["securityContext"]["runAsNonRoot"] is True
        for container in spec["containers"]:
            assert container["securityContext"]["readOnlyRootFilesystem"] is True
            assert container["securityContext"]["capabilities"]["drop"] == ["ALL"]
            assert "@sha256:" in container["image"]
            assert container["resources"]["limits"]["memory"] in ("64Mi", "1Gi")
    one(dev, "Service", "basemap-tiles")
    routes = yaml.safe_load((COMPONENT / "apisix-routes.yaml").read_text().replace("{{", "#{{"))
    assert routes["basemap"]["subDomain"] == "tiles" and routes["basemap"]["uri"] == "/*"
    # The host is on the edge certificate (HTTP-01, one name among the others).
    certificate = one(dev, "Certificate", "apisix-edge")
    assert HOST in certificate["spec"]["dnsNames"]


def test_the_style_asks_no_other_host():
    files = sorted(STYLES.glob("light-*.json"))
    assert [f.name for f in files] == ["light-cs.json", "light-en.json", "light-fi.json", "light-sk.json"]
    for path in files:
        style = json.loads(path.read_text())
        assert style["glyphs"] == "https://__HOST__/fonts/{fontstack}/{range}.pbf"
        assert style["sprite"] == "https://__HOST__/sprites/v4/light"
        source = style["sources"]["protomaps"]
        assert source["url"] == "pmtiles://https://__HOST__/__FILE__"
        attribution = source.pop("attribution")
        assert "OpenStreetMap contributors" in attribution and "Protomaps" in attribution
        # Every other address the style holds is on our host; the attribution's are links.
        foreign = {u for u in re.findall(r"[a-z]+://[^\"\\ ]+", json.dumps(style)) if "__HOST__" not in u}
        assert foreign == set(), path.name
        lang = path.stem.split("-")[1]
        assert f'"name:{lang}"' in path.read_text(), "labels in the style's language"


def test_the_rendered_style_names_the_tile_host(dev):
    styles = one(dev, "ConfigMap", "basemap-tiles-styles")["data"]
    sk = json.loads(styles["light-sk.json"])
    assert sk["sources"]["protomaps"]["url"] == f"pmtiles://https://{HOST}/basemap.pmtiles"
    assert "__HOST__" not in json.dumps(styles)


def test_the_server_is_read_only_and_shares_with_our_hosts_alone(dev):
    conf = one(dev, "ConfigMap", "basemap-tiles")["data"]["default.conf"]
    assert '"~^https://([a-z0-9-]+\\.)*dev\\.joinedcontext\\.com$" $http_origin;' in conf
    origin = re.compile(r"^https://([a-z0-9-]+\.)*dev\.joinedcontext\.com$")
    assert origin.match("https://opendata.dev.joinedcontext.com")
    assert not origin.match("https://evildev.joinedcontext.com")
    assert not origin.match("https://dev.joinedcontext.com.evil.example")
    assert conf.count("limit_except GET") == 2, "GET and HEAD (and the preflight) only"
    assert 'add_header Cache-Control "public, max-age=86400" always;' in conf
    assert "autoindex off;" in conf and "location ~ /\\. { return 404; }" in conf


def test_the_regions_become_one_closed_multipolygon(dev):
    container = one(dev, "CronJob", "basemap-tiles")["spec"]["jobTemplate"]["spec"]["template"]["spec"]["containers"][0]
    env = {e["name"]: e["value"] for e in container["env"]}
    region = json.loads(env["REGION"])
    assert region["type"] == "MultiPolygon" and len(region["coordinates"]) == 3
    for polygon in region["coordinates"]:
        ring = polygon[0]
        assert len(ring) == 5 and ring[0] == ring[-1]
        (w, s), (e, _), (_, n) = ring[0], ring[1], ring[2]
        assert -180 <= w < e <= 180 and -90 <= s < n <= 90
    assert env["MAXZOOM"] == "15" and env["FILE"] == "basemap.pmtiles"
    assert re.fullmatch(r"[0-9a-f]{64}", env["PMTILES_SHA256"])
    assert re.fullmatch(r"[0-9a-f]{64}", env["ASSETS_SHA256"])


def test_the_refresh_checks_the_room_and_swaps_by_rename(dev):
    script = one(dev, "ConfigMap", "basemap-tiles")["data"]["refresh.sh"]
    order = [
        'sha256sum -c -',               # the CLI before it runs
        '--dry-run',                    # the size before a byte is written
        'df -P "$DATA"',                # the room, printed
        's * 1.2',                      # plus 20 %
        'if [ "$need" -gt "$free" ]',   # refused, old file kept
        'tmp="${DATA}/.${FILE}.tmp"',
        './pmtiles verify "$tmp"',
        'mv -f "$tmp" "${DATA}/${FILE}"',
    ]
    at = [script.index(step) for step in order]
    assert at == sorted(at), "each step after the one before it"
    assert script.startswith("#!/bin/sh") and "set -eu" in script


def test_the_refresh_reaches_only_public_addresses():
    """The refresh job fetches from the internet on 443, never inside the cluster or the node's
    networks (the CKAN harvest's rule): its open egress block leaves out every private range."""
    policies = yaml.safe_load((COMPONENT / "networkpolicies.yaml").read_text())
    peers = [peer for rule in policies["basemap-refresh"]["egress"] for peer in rule["to"]]
    # The shared `publicInternet` peer (components/networkpolicies, F-R2DEP2-023) excepts every
    # private range; no other address range is named.
    assert {"publicInternet": True} in peers, "the refresh job has no internet egress"
    assert not [peer for peer in peers if "ipBlock" in peer], peers
