"""T-4740: the BBSK demo's host serves a landing page with the demo assistant docked, not a CKAN.

The page is static: a snapshot of the www.bbsk.sk home page (owner 2026-10-19T12:55Z: its texts and
layout, no logo, photos as colour blocks, links to the real pages) with the assistant, its proven
questions, the MCP block and the AI notice added, plus the catalogue theme's dock scripts, on the
platform's pinned nginx. It says DEMO in its ribbon, title and footer, keeps search engines out,
loads no file of another host and frames only the demo assistant.
"""

import shutil
import socket
import subprocess
import time
import urllib.error
import urllib.request
from html.parser import HTMLParser
from pathlib import Path

import pytest
import yaml
from test_demo_bbsk import HOSTS, one, render

pytestmark = pytest.mark.xdist_group("docker-demo-bbsk-landing")

ROOT = Path(__file__).resolve().parent.parent
THEME = ROOT / "charts/ckan/files/jc_theme"
RIBBON = {"index.html": "DEMO · ukážka od joinedcontext, nie oficiálna stránka BBSK"}


@pytest.fixture(scope="module", params=sorted(HOSTS))
def landing(request):
    env = request.param
    docs = render(env)
    site = one(docs, "ConfigMap", "demo-bbsk-landing-site")["data"]
    conf = one(docs, "ConfigMap", "demo-bbsk-landing")["data"]["default.conf"]
    domain = HOSTS[env].split(".", 1)[1].removeprefix("apps.")
    return env, domain, docs, site, conf


class Page(HTMLParser):
    """What a page loads (scripts, styles, frames, images) and the text it shows."""

    def __init__(self):
        super().__init__()
        self.loads, self.frames, self.meta, self.text, self.links = [], [], {}, [], []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == "script":
            self.loads.append(a.get("src"))
        elif tag == "link" and a.get("rel") == "stylesheet":
            self.loads.append(a["href"])
        elif tag in ("img", "source", "video", "audio", "object", "embed"):
            self.loads.append(a.get("src") or a.get("data") or "")
        elif tag == "iframe":
            self.frames.append(a["src"])
        elif tag == "a":
            self.links.append(a)
        elif tag == "meta" and "name" in a:
            self.meta[a["name"]] = a["content"]

    def handle_data(self, data):
        self.text.append(data)


def parse(html: str) -> Page:
    page = Page()
    page.feed(html)
    return page


def test_the_page_is_static_on_the_pinned_nginx_and_closed_but_for_the_edge(landing):
    _, _, docs, _, _ = landing
    pod = one(docs, "Deployment", "demo-bbsk-landing")["spec"]["template"]["spec"]
    nginx = pod["containers"][0]
    assert nginx["image"].startswith("docker.io/nginxinc/nginx-unprivileged:") and "@sha256:" in nginx["image"]
    assert nginx["securityContext"]["readOnlyRootFilesystem"] is True
    assert pod["securityContext"]["runAsNonRoot"] is True and pod["automountServiceAccountToken"] is False
    policy = one(docs, "NetworkPolicy", "demo-bbsk-landing")["spec"]
    assert policy["podSelector"] == {"matchLabels": {"app.kubernetes.io/name": "demo-bbsk-landing"}}
    assert policy["ingress"][0]["ports"] == [{"protocol": "TCP", "port": 8080}]


def test_the_page_says_demo_keeps_search_engines_out_and_loads_nothing_third_party(landing):
    _, domain, _, site, _ = landing
    for name, ribbon in RIBBON.items():
        page = parse(site[name])
        text = "".join(page.text)
        assert text.count(ribbon) == 2, name  # the ribbon and the footer
        assert ribbon.split(" · ")[0] in site[name].split("<title>")[1].split("</title>")[0], name
        assert page.meta["robots"] == "noindex, nofollow", name
        # Every script and style is this host's own; the one frame is the demo assistant.
        assert page.loads and all(src and src.startswith("/") for src in page.loads), (name, page.loads)
        assert page.frames == [f"https://assistant.{domain}/d/bbsk-demo/widget?embed=1&dock=1&lang=sk&scheme=light"], name
        assert f"https://api.{domain}/api/mcp/d/bbsk-demo" in text, name
        assert "<form" not in site[name] and "__DOMAIN__" not in site[name], name
        # No coat of arms, no logo and no file of bbsk.sk: their site is only linked.
        assert "<img" not in site[name] and "<picture" not in site[name], name
        assert "bbsk.sk/themes" not in site[name] and "bbsk.sk/storage" not in site[name], name


def test_the_cards_are_the_demo_assistants_own_questions_and_the_panel_speaks_slovak(landing):
    _, _, _, site, _ = landing
    seed = yaml.safe_load((ROOT / "components/context-gateway/seed/bbsk/bbsk-assistant-demo.yaml").read_text())
    for name, lang in (("index.html", "sk"),):
        cards = [part.split("</strong>")[0] for part in site[name].split('<button type="button" data-ask><span class="head">')[1:]]
        cards = [card.split("<strong>")[1] for card in cards]
        assert cards == seed["spec"]["suggestions"][lang], name
        # The page is Slovak, so the panel opens in Slovak whatever the browser says (owner 13:50Z item 3).
        assert "/d/bbsk-demo/widget?embed=1&amp;dock=1&amp;lang=sk&amp;scheme=light" in site[name], name


def test_docked_the_page_scrolls_beside_the_panel_not_behind_it(landing):
    """T-4767: one scrollbar at the screen's edge, the chat's; the page's own stands beside the panel."""
    _, _, _, site, _ = landing
    dock = site["landing.css"].split("@media (min-width: 1100px) {", 1)[1].split("\n}\n", 1)[0]
    assert "html.jc-docked { --jc-dock-w: clamp(360px, var(--jc-dock, 34vw), 50vw); overflow: hidden; }" in dock
    assert "html.jc-docked body { margin-right: var(--jc-dock-w); height: 100vh; height: 100dvh; overflow-y: auto; scroll-padding-top: calc(var(--ribbon) + 1rem); }" in dock


def test_the_dock_is_the_catalogues_own_and_the_policy_frames_only_the_assistant(landing):
    _, domain, _, site, conf = landing
    for script in ("jc-dock.js", "jc-assistant.js"):
        assert site[script] == (THEME / f"public__{script}").read_text(), script
    csp = next(line for line in conf.splitlines() if "Content-Security-Policy" in line)
    assert "script-src 'self';" in csp and "style-src 'self';" in csp and "default-src 'none';" in csp
    assert f"frame-src https://assistant.{domain};" in csp and "frame-ancestors 'none'" in csp
    assert 'X-Robots-Tag "noindex, nofollow"' in conf


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


@pytest.fixture(scope="module")
def served(tmp_path_factory):
    """The smartcity render, served by its own pinned image as the cluster runs it."""
    if not shutil.which("docker"):
        pytest.skip("docker not installed")
    docs = render("smartcity")
    tmp = tmp_path_factory.mktemp("landing")
    (tmp / "site").mkdir()
    for name, body in one(docs, "ConfigMap", "demo-bbsk-landing-site")["data"].items():
        (tmp / "site" / name).write_text(body)
    (tmp / "default.conf").write_text(one(docs, "ConfigMap", "demo-bbsk-landing")["data"]["default.conf"])
    for path in [tmp, tmp / "site", *tmp.rglob("*")]:
        path.chmod(0o755 if path.is_dir() else 0o644)
    image = one(docs, "Deployment", "demo-bbsk-landing")["spec"]["template"]["spec"]["containers"][0]["image"]
    port = free_port()
    name = f"demo-bbsk-landing-{port}"
    started = subprocess.run(
        ["docker", "run", "-d", "--name", name, "--read-only", "--tmpfs", "/tmp", "--tmpfs", "/var/cache/nginx",
         "-u", "101:101", "-p", f"127.0.0.1:{port}:8080",
         "-v", f"{tmp / 'default.conf'}:/etc/nginx/conf.d/default.conf:ro", "-v", f"{tmp / 'site'}:/srv/site:ro", image],
        capture_output=True, text=True, check=False)
    if started.returncode != 0:
        pytest.skip(f"the pinned image does not run here: {started.stderr.strip()}")
    base = f"http://127.0.0.1:{port}"
    try:
        deadline = time.time() + 30
        while time.time() < deadline:
            try:
                urllib.request.urlopen(base + "/healthz", timeout=2)
                break
            except OSError:
                time.sleep(0.3)
        else:
            pytest.skip("nginx did not answer within 30 s")
        yield base
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)


class Stay(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


def fetch(url: str, method: str = "GET"):
    opener = urllib.request.build_opener(Stay)
    try:
        with opener.open(urllib.request.Request(url, method=method), timeout=10) as answer:
            return answer.status, answer.headers, answer.read().decode()
    except urllib.error.HTTPError as error:
        return error.code, error.headers, ""


def test_the_served_page_answers_with_noindex_and_nothing_else_is_served(served):
    status, headers, body = fetch(served + "/")
    assert status == 200 and "text/html" in headers["Content-Type"] and RIBBON["index.html"] in body
    assert headers["X-Robots-Tag"] == "noindex, nofollow" and "frame-ancestors 'none'" in headers["Content-Security-Policy"]
    assert "Set-Cookie" not in headers
    for path in ("/landing.css", "/landing.js", "/jc-dock.js", "/jc-assistant.js"):
        assert fetch(served + path)[0] == 200, path
    for path in ("/index.html", "/en/", "/default.conf", "/.git", "/admin", "/api/3/action/package_list"):
        assert fetch(served + path)[0] == 404, path
    assert fetch(served + "/", "POST")[0] == 403


def test_the_snapshot_keeps_bbsk_sks_texts_links_to_its_pages_and_shows_no_logo(landing):
    _, domain, _, site, _ = landing
    page = parse(site["index.html"])
    text = " ".join(page.text)
    # The home page's own sections and texts, as fetched on 2026-10-19.
    for words in ("O kraji", "Aktuality", "Dôležité", "Služby", "Sociálne služby", "Kalendár", "Možno hľadáte",
                  "Viac o BBSK", "O stránke", "Námestie SNP 23"):
        assert words in text, words
    # Their links go to the real pages, never to a copy here.
    theirs = [a for a in page.links if a.get("href", "").startswith("https://www.bbsk.sk")]
    assert len(theirs) > 50 and all(a.get("rel") == "noopener" for a in theirs)
    assert all(a.get("href", "").startswith(("https://", "#")) for a in page.links), "no relative link to a page this host lacks"
    # Ours is marked as ours, and the name stands where the logo was.
    assert text.count("DEMO · pridané joinedcontext") == 4  # the hero, the cards, the calendar, the MCP block
    # The calendar card opens the region's events dashboard on its App host (T-4897).
    calendar = [a for a in page.links if "calendar" in a.get("class", "")]
    assert [a.get("href") for a in calendar] == [f"https://bbsk-podujatia.apps.{domain}/"]
    assert "Banskobystrický samosprávny kraj · DEMO" in text


def test_the_launcher_and_the_dock_edge_never_show_a_red_focus_ring():
    # T-4761 (owner 2026-10-19T19:59Z, "this red does not looks good"): closing the panel gives
    # focus back to the launcher, which drew the page's red focus ring. The ring stays (WCAG 2.4.7)
    # in white with a dark blue edge.
    css = (Path(__file__).resolve().parent.parent / "demos/bbsk/chart/files/landing/landing.css").read_text()
    rules = dict(
        (selector.strip(), body)
        for selector, body in (line.split("{", 1) for line in css.splitlines() if ":focus-visible" in line and "{" in line)
    )
    launcher = rules[".jc-assistant > summary:focus-visible"]
    assert "outline: 3px solid var(--white)" in launcher and "var(--blue-dark)" in launcher
    assert "var(--red)" not in rules[".jc-dock-edge:focus-visible"]


def test_the_page_passes_axe_s_heading_contrast_link_and_landmark_rules():
    """T-4996 (axe on dev, journey 31): the page has one h1, the hero's DEMO tag is white on red
    (light blue on red was 3.6:1), a link in running text is underlined, so it reads as a link
    without its colour (1.8:1 against the notice's text), and the DEMO ribbon sits in the header
    landmark. axe-core 4.13 on the page served from the folder: 0 violations at 1440 and 400."""
    folder = ROOT / "demos/bbsk/chart/files/landing"
    html = (folder / "index.html").read_text()
    css = (folder / "landing.css").read_text()
    assert html.count("<h1") == 1
    assert html.index("<h1>AI asistent nad dátami vášho kraja") < html.index("<h2", html.index("<main"))
    assert ".hero__ai h1 { color: var(--white);" in css and ".hero__ai h2" not in css
    assert ".hero__ai .ai__tag" not in css, "the tag keeps .ai__tag's white on red"
    assert "p a { text-decoration: underline;" in css
    header = html.split('<header class="header">', 1)[1].split("</header>", 1)[0]
    assert '<div class="ribbon" role="note">' in header
    assert html.count('<div class="ribbon"') == 1
