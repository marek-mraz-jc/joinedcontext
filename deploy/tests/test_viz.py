"""T-4641, ADR-N-053: the chart renderer on viz.{domain}. The rendered dev environment carries
the server, its files and the pinned Vega builds, the route and its certificate name; each page
carries the headers that keep a spec harmless; the browser code's own tests run under node."""

import base64
import gzip
import hashlib
import re
import subprocess
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
COMPONENT = ROOT / "components/viz"
FILES = COMPONENT / "charts/renderer/files"
VENDOR = ROOT / "images/viz/vendor"
HOST = "viz.dev.joinedcontext.com"


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"expected one {kind}/{name}, got {len(found)}"
    return found[0]


def test_dev_renders_a_read_only_server_and_its_route_on_its_own_host(dev):
    pod = one(dev, "Deployment", "viz-renderer")["spec"]["template"]["spec"]
    assert pod["automountServiceAccountToken"] is False and pod["securityContext"]["runAsNonRoot"] is True
    container = pod["containers"][0]
    assert "@sha256:" in container["image"]
    assert container["securityContext"]["readOnlyRootFilesystem"] is True
    assert container["securityContext"]["capabilities"]["drop"] == ["ALL"]
    assert all(m["readOnly"] for m in container["volumeMounts"] if m["name"] in ("config", "site"))
    one(dev, "Service", "viz-renderer")
    routes = yaml.safe_load((COMPONENT / "apisix-routes.yaml").read_text().replace("{{", "#{{"))
    assert routes["viz"]["subDomain"] == "viz" and routes["viz"]["uri"] == "/*"
    assert HOST in one(dev, "Certificate", "apisix-edge")["spec"]["dnsNames"]
    # Any site may frame the host page (the embed snippet): the edge adds no X-Frame-Options.
    plugins = yaml.safe_load((COMPONENT / "apisix-plugins.yaml").read_text())["viz"]["plugins"]
    assert "X-Frame-Options" not in plugins["response-rewrite"]["headers"]["set"]


# The npm builds the two gzipped MapLibre modules unpack to (T-4657), as SHA256SUMS notes them.
UNPACKED = {
    "maplibre-gl-6.14.0.mjs.gz": "244d36122c1398da3a8259817cc6d71e04321afe6404f32606b4b6c6cc4f26fb",
    "maplibre-gl-worker-6.14.0.mjs.gz": "5f1e2f0e1624082b7e88a06e4c86085992b0f8c02929801e552f3f7a119856a9",
}


def test_the_vendored_files_are_the_releases_byte_for_byte_and_ride_in_the_image():
    sums = [line.split() for line in (VENDOR / "SHA256SUMS").read_text().splitlines() if line and not line.startswith("#")]
    assert sorted(name for _, name in sums) == sorted(p.name for p in VENDOR.iterdir() if p.name != "SHA256SUMS")
    for digest, name in sums:
        assert hashlib.sha256((VENDOR / name).read_bytes()).hexdigest() == digest, name
    for name, digest in UNPACKED.items():
        assert hashlib.sha256(gzip.decompress((VENDOR / name).read_bytes())).hexdigest() == digest, name
        assert digest in (VENDOR / "SHA256SUMS").read_text()
    pages = (FILES / "site/render.html").read_text() + (FILES / "site/map.html").read_text() + (FILES / "site/map.js").read_text()
    for _, name in sums:
        assert name.removesuffix(".gz") in pages, name
    # The image checks the same sums when it is built, so a changed file fails the build.
    dockerfile = (VENDOR.parent / "Dockerfile").read_text()
    assert "COPY vendor/ /srv/vendor/" in dockerfile and "sha256sum -c" in dockerfile
    assert "alias /srv/vendor/;" in (FILES / "nginx.conf").read_text()
    # No page loads a script from anywhere but this host.
    for path in (FILES / "site").glob("*.html"):
        assert not re.search(r"(src|href)=\"(https?:)?//", path.read_text()), path.name


def test_the_release_stays_well_inside_the_secret_helm_stores_it_in(dev):
    """T-4657: Helm keeps a release (its chart files and the rendered manifest, gzipped, base64)
    in one Secret of at most 1 MiB; the vendored builds took the renderer to 2.2 MB. Under
    0.75 MiB, as the seed releases are held (test_sample_apps_seed)."""
    chart = FILES.parent
    files = b"".join(p.read_bytes() for p in sorted(chart.rglob("*")) if p.is_file())
    manifest = yaml.safe_dump_all(
        [d for d in dev if d.get("metadata", {}).get("labels", {}).get("app.kubernetes.io/instance") == "viz-renderer"]
    ).encode()
    assert b"viz-renderer" in manifest
    stored = len(base64.b64encode(gzip.compress(files + manifest)))
    assert stored < 0.75 * 1024 * 1024, f"the viz-renderer release would be stored in {stored} bytes"


def csp(conf, location):
    block = conf[conf.index(f"location = {location} {{"):]
    return re.search(r'Content-Security-Policy "([^"]+)"', block).group(1)


def test_each_page_carries_the_headers_that_keep_a_spec_harmless(dev):
    conf = one(dev, "ConfigMap", "viz-renderer")["data"]["default.conf"]
    host = csp(conf, "/")
    assert "connect-src https://api.dev.joinedcontext.com https://opendata.dev.joinedcontext.com;" in host
    assert f"script-src https://{HOST};" in host and "frame-ancestors *" in host
    render = csp(conf, "/render.html")
    assert "connect-src 'none'" in render, "the frame that runs a spec calls no host"
    assert f"script-src https://{HOST};" in render
    for policy in (host, render):
        assert "unsafe-eval" not in policy and "default-src 'none'" in policy and "base-uri 'none'" in policy
        assert "'unsafe-inline'" not in policy.split("script-src")[1].split(";")[0]
    assert "Set-Cookie" not in conf and "add_header Referrer-Policy no-referrer always;" in conf
    assert conf.count("limit_except GET") >= 3
    page = (FILES / "site/index.html").read_text()
    assert 'sandbox="allow-scripts allow-downloads"' in page, "the renderer runs in an opaque origin"
    # T-4657: the map page reaches the tiles and nothing else, and MapLibre's modules leave as .gz.
    mapped = csp(conf, "/map.html")
    assert "connect-src https://tiles.dev.joinedcontext.com;" in mapped and "unsafe-eval" not in mapped
    assert f"script-src https://{HOST};" in mapped and "worker-src blob: https://" + HOST in mapped
    assert "gzip_static always;" in conf and "text/javascript js mjs;" in conf
    # T-4711: scripts and styles leave gzipped, and a versioned vendor file is cached for good.
    assert "gzip on;" in conf and "gzip_types text/javascript application/javascript text/css application/json" in conf
    vendor = conf.split("location /vendor/")[1]
    assert 'add_header Cache-Control "public, max-age=31536000, immutable" always;' in vendor
    assert "max-age=86400" not in vendor
    # T-5059: the explorer reads the API and the tiles, and only the catalogue may frame it.
    explore = csp(conf, "/explore.html")
    assert "connect-src https://api.dev.joinedcontext.com https://tiles.dev.joinedcontext.com;" in explore
    assert "frame-ancestors https://opendata.dev.joinedcontext.com" in explore and "frame-ancestors *" not in explore
    assert f"script-src https://{HOST};" in explore and "unsafe-eval" not in explore and "base-uri 'none'" in explore
    # T-5129: /embed.html is the same page for any site: framed by anyone, reading the API, the tiles and
    # the catalogue's dataset for its source line, and nothing else.
    embed = csp(conf, "/embed.html")
    assert "try_files /explore.html =404;" in conf.split("location = /embed.html {")[1].split("location")[0]
    assert "connect-src https://api.dev.joinedcontext.com https://tiles.dev.joinedcontext.com https://opendata.dev.joinedcontext.com;" in embed
    assert "frame-ancestors *" in embed and f"script-src https://{HOST};" in embed and "unsafe-eval" not in embed and "base-uri 'none'" in embed
    assert embed.replace(" https://opendata.dev.joinedcontext.com;", ";").replace("frame-ancestors *", "frame-ancestors https://opendata.dev.joinedcontext.com") == explore
    embedjs = (FILES / "site/embed.js").read_text()
    assert "innerHTML" not in embedjs and "allow-top-navigation" not in embedjs and "allow-forms" not in embedjs
    assert 'event.origin === origin && event.source === frame.contentWindow' in embedjs
    explorejs = (FILES / "site/explore.js").read_text()
    assert 'credentials: "omit"' in explorejs and "innerHTML" not in explorejs and "Authorization" not in explorejs
    assert "datastore" not in explorejs
    mapped_page = (FILES / "site/map.html").read_text()
    assert '<link rel="modulepreload" href="/vendor/maplibre-gl-6.14.0.mjs">' in mapped_page
    assert "maplibre-gl-6.14.0.mjs" in (FILES / "site/map.js").read_text(), "the preload names the module map.js imports"


def test_the_renderer_draws_through_the_interpreter_and_loads_nothing():
    render = (FILES / "site/render.js").read_text()
    assert "ast: true" in render and "expr: vega.expressionInterpreter" in render
    # GHSA-m9rg-mr6g-75gm and GHSA-7f2v-3qq3-vvjf (HIGH): Vega before 6.2.0 runs a window function
    # an expression reaches. The interpreter is scripts/viz-vega-interpreter.sh over the release.
    (vega,) = [p.name for p in VENDOR.glob("vega-[0-9]*.min.js")]
    assert tuple(int(n) for n in vega.split("-")[1].removesuffix(".min.js").split(".")) >= (6, 2, 0), vega
    (interpreter,) = VENDOR.glob("vega-interpreter-*.js")
    text = interpreter.read_text()
    assert text.startswith("(function (vega) {") and "vega.expressionInterpreter = expression;" in text
    assert "import " not in text.split("\n", 3)[2] and text.rstrip().endswith("})(self.vega);")
    assert "editor: false" in render and "source: false" in render
    assert "loader: loader" in render and "self.jcRefuse(message.spec" in render
    assert "event.source !== window.parent || event.origin !== host" in render
    hostjs = (FILES / "site/host.js").read_text()
    assert 'credentials: "omit"' in hostjs and "root.jcRefuse(spec" in hostjs
    # `#show` takes a spec only from a parent on our hosts, and checks it before any frame sees it.
    assert "ourOrigin(event.origin, h.domain)" in hostjs and "root.jcRefuseMap(data.spec, h.domain)" in hostjs
    mapjs = (FILES / "site/map.js").read_text()
    assert "innerHTML" not in mapjs and "root.jcRefuseMap(spec, domain)" in mapjs
    assert "event.source !== window.parent || event.origin !== location.origin" in mapjs


EXPLORER = ROOT / "images/viz/explorer"


def test_the_explorer_table_is_one_portal_commit_and_rides_in_the_image():
    # T-5082: explore.html's table is the Portal's Endpoint data view, built by
    # scripts/viz-explorer-bundle.sh; SHA256SUMS names the Portal commit and every file's sum.
    text = (EXPLORER / "SHA256SUMS").read_text()
    assert re.match(r"# portal: [0-9a-f]{40}\n", text)
    sums = [line.split() for line in text.splitlines() if line and not line.startswith("#")]
    assert sorted(name for _, name in sums) == ["explorer.css", "explorer.js"]
    assert sorted(p.name for p in EXPLORER.iterdir()) == ["SHA256SUMS", "explorer.css", "explorer.js"]
    for digest, name in sums:
        assert hashlib.sha256((EXPLORER / name).read_bytes()).hexdigest() == digest, name
    html = (FILES / "site/explore.html").read_text()
    assert '<script src="explorer/explorer.js" defer></script>' in html
    assert html.index("explorer/explorer.js") < html.index('src="explore.js"')
    dockerfile = (VENDOR.parent / "Dockerfile").read_text()
    assert "COPY explorer/ /srv/explorer/" in dockerfile and "cd /srv/explorer && grep -v '^#' SHA256SUMS | sha256sum -c -" in dockerfile
    assert "alias /srv/explorer/;" in (FILES / "nginx.conf").read_text()
    # The view reads the API without a cookie: no credentials: "include" anywhere in the bundle.
    assert not re.search(r"credentials:\s*[`\"']include", (EXPLORER / "explorer.js").read_text())


def test_the_refusal_list_and_the_host_page_logic_under_node():
    tests = sorted(str(p) for p in (COMPONENT / "tests").glob("*.test.mjs"))
    assert len(tests) == 5
    run = subprocess.run(["node", "--test", *tests], capture_output=True, text=True, timeout=120)
    assert run.returncode == 0, run.stdout[-4000:] + run.stderr[-2000:]


def test_the_assistant_draws_its_maps_and_charts_on_this_host(dev):
    # T-4657: with the viz component on, the assistant knows the renderer's origin, so its data
    # answers get maps and charts and its widget page may frame this host alone.
    pod = one(dev, "Deployment", "jc-assistant")["spec"]["template"]["spec"]
    env = {e["name"]: e.get("value") for c in pod["containers"] for e in c.get("env", [])}
    assert env["JC_ASSISTANT_VIZ_URL"] == f"https://{HOST}"


def test_every_location_with_headers_of_its_own_repeats_the_shared_ones():
    """nginx drops the server's add_header set in a location with one of its own: the pages lost
    Cross-Origin-Opener-Policy and Permissions-Policy (F-R2DEP3-020, curl -I viz.dev, 2026-10-31)."""
    conf = (FILES / "nginx.conf").read_text()
    server_level = [l.strip() for l in conf.split("location", 1)[0].splitlines() if l.startswith("    add_header")]
    server_level += [l.strip() for l in conf.split("location = /healthz")[1].split("}", 1)[1].split("location")[0].splitlines()
                     if l.startswith("    add_header")]
    shared = [l for l in server_level if not l.startswith(("add_header Content-Security-Policy", "add_header Cache-Control"))]
    assert len(shared) == 4, shared
    for block in re.findall(r"\n    location ([^{]+)\{(.*?)\n    \}", conf, re.S):
        name, body = block
        if "add_header" in body:
            missing = [h for h in shared if h not in body]
            assert not missing, f"location {name.strip()} drops {missing}"
