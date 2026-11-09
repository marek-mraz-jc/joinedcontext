"""Smoke suite `ui`: the Portal live walker over a jc, signed in as each persona, plus the other UIs.

    bins/jc/smoke/run ui --base-url http://localhost:42101 --admin-token-file <f> --out <dir>
        [--export <export dir>] [--project <p>] [--widths 1440,400] [--only <regex>]
        [--shako-token-file <f>] [--judge-file <scores.json>] [--personas admin,viewer,outsider]

Per persona (personas.py: admin, viewer, outsider) the suite signs in through the Portal's own
login at 1440 px, gathers every route the live walker gathers (walk.py, a port of
crates/portal/ui/e2e/live/walk-routes.ts for the persona's project) and opens each address once,
judging it at every width by resizing the open page, as walker.spec.ts does. A page fails on:

- any answer >= 500 the page asked for (document, API, asset), and an /api/ answer >= 400 the
  person should not meet (walker.spec.ts `expected`, plus 403 for the viewer and the outsider);
- a console error or an uncaught page error;
- every finding of pageChecks.ts (one h1, purpose line, empty state with a next step, field hints,
  error text, raw i18n key, raw JSON, bare URN, row links, sideways overflow), taken from the
  Portal source at run time (checks.py), with walker.allow.json's excuses honoured and named;
- `table-urn`: a table cell that is a bare URN outside an id column;
- `viewer-write` for the viewer and the outsider: a write control they can press, or one refused
  without a reason.

Then, anonymously and as the admin: dataroom (/opendata: home, dataset list, the first dataset,
the first organization; on the first dataset with a "live data" frame, `viz` rows: /viz/explore.html
and /viz/embed.html answer a CSP sandbox without allow-same-origin when viz shares the Portal's
origin, the framed explorer runs in an opaque origin that reads no cookie of the host, and its grid
draws rows), shako (/git: the repository list and the first repository, with
--shako-token-file; a browser person without one is reported), and the daiya dashboards the
Portal lists for the project (/projects/{p}/grafana and every dashboard link on it).

Each page and width gets a screenshot (shots/<persona>/<width>/<name>.png, whole height in the
viewport, so the sticky sidebar is not cut) and a row in shots.json, the input of the UI judge
(JUDGE.md). `--judge-file` folds a judge's scores in: a page passes the judge with every score
>= 4 and no high finding; a page the judge did not score is a skip.

Expectations come from data: personas and projects from the export's RoleBindings and
project.yaml; dashboards from the export's grafana/dashboards; the routes from the Portal router.
Nothing is written but the test persons (and a missing group a binding names) in realm `jc`."""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))

import checks  # noqa: E402
import personas as ps  # noqa: E402
import walk  # noqa: E402
from common import FAIL, SKIP, Http, Results, loopback_only, read_token  # noqa: E402

LOGIN = "/api/v1/auth/login"
HEIGHT = {400: 860, 1440: 900}


def expected_4xx(who: str, status: int, method: str, url: str) -> bool:
    """A 4xx a person may meet with nothing wrong: walker.spec.ts `expected`, plus a 403 for any
    persona that is not an administrator (the outsider is refused what the viewer is)."""
    path = url.split("?", 1)[0]
    if status == 403 and who != "admin":
        return True
    if status == 401 and who == "anonymous":
        return True
    # A project the outsider holds nothing in is hidden from them: its reads answer 404.
    if status == 404 and who == "outsider" and "/api/v1/projects/" in path:
        return True
    if status == 404 and re.search(r"/cs/[^/]+/ngsi-ld/v1/entities\?", url):
        return True
    if status == 404 and method == "GET" and re.search(r"/drafts/[A-Za-z]+/[^/?]+$", path):
        return True
    return status == 410 or (status == 404 and re.search(r"/agent-runs/[^/]+/(preview|events)", url) is not None)


class Walker:
    def __init__(self, a, browser, res: Results, out: Path):
        self.a, self.browser, self.res, self.out = a, browser, res, out
        self.base = a.base_url.rstrip("/")
        self.script = checks.script()
        self.namespaces = checks.namespaces()
        self.allow = checks.excused()
        self.shots: list[dict] = []
        self.current: list[str] = []

    # -- one browser context per persona -------------------------------------------------------
    def context(self, who: str, headers: dict | None = None):
        ctx = self.browser.new_context(viewport={"width": 1440, "height": HEIGHT[1440]},
                                       extra_http_headers=headers or {})
        ctx.add_init_script(self.script)
        page = ctx.new_page()

        def on_response(r):
            if r.status >= 500:
                self.current.append(f"http-5xx: {r.status} {r.request.method} {self.rel(r.url)}")
            elif r.status >= 400 and "/api/" in r.url and not expected_4xx(who, r.status, r.request.method, r.url):
                self.current.append(f"http: {r.status} {r.request.method} {self.rel(r.url).split('?')[0]}")

        def on_console(m):
            if m.type == "error" and not m.text.startswith("Failed to load resource"):
                self.current.append(f"console: {m.text[:200]}")

        page.on("response", on_response)
        page.on("console", on_console)
        page.on("pageerror", lambda e: self.current.append(f"console: {str(e)[:200]}"))
        return ctx, page

    def rel(self, url: str) -> str:
        return url[len(self.base):] if url.startswith(self.base) else url

    def sign_in(self, page, who: str, username: str, password: str) -> tuple[bool, str]:
        page.goto(self.base + LOGIN, wait_until="load", timeout=60000)
        try:
            page.fill("input[name=username]", username, timeout=20000)
            page.fill("input[name=password]", password)
            page.locator("input[name=password]").press("Enter")
            page.wait_for_url(re.compile(re.escape(self.base) + r"/(?!id/).*"), timeout=30000)
        except Exception as e:  # noqa: BLE001 - the row says what the page did
            return False, f"{type(e).__name__}: {str(e)[:160]} at {self.rel(page.url)}"
        st = page.request.get(self.base + "/api/v1/auth/me")
        return st.status == 200, f"/api/v1/auth/me {st.status} at {self.rel(page.url)}"

    # -- one address, judged at every width ------------------------------------------------------
    def judge(self, page, who: str, area: str, route: str, address: str, *, portal: bool, viewer: bool):
        self.current = []
        call = f"GET {address} as {who}"
        page.set_viewport_size({"width": self.a.widths[0], "height": HEIGHT.get(self.a.widths[0], 900)})
        doc_status = None
        try:
            resp = page.goto(self.base + address, wait_until="load", timeout=60000)
            doc_status = resp.status if resp else None
            try:
                page.wait_for_load_state("networkidle", timeout=15000)
            except Exception:  # noqa: BLE001 - a page that keeps polling still gets judged
                pass
            if portal:
                page.locator("main h1:visible").first.wait_for(timeout=15000)
        except Exception as e:  # noqa: BLE001
            self.current.append(f"load: {type(e).__name__}: {str(e).splitlines()[0][:160]}")
        loaded = list(self.current)
        for i, width in enumerate(self.a.widths):
            findings = loaded if i == 0 else []
            if i:
                self.current = []
                page.set_viewport_size({"width": width, "height": HEIGHT.get(width, 900)})
                page.wait_for_timeout(400)
                findings = list(self.current)
            if doc_status is not None and doc_status >= 400:
                findings.append(f"document: {doc_status}")
            findings += self.dom(page, portal=portal, viewer=viewer)
            shot = self.screenshot(page, who, width, address)
            kept, excused = [], []
            for f in dict.fromkeys(findings):
                e = checks.excuse(route, f, self.allow) if portal else None
                (excused if e else kept).append(f"{f} (excused: {e['task']})" if e else f)
            h1 = self.h1(page)
            self.shots.append({"page": f"{area}:{who}:{address}", "area": area, "persona": who, "route": route,
                               "address": address, "width": width, "status": doc_status, "h1": h1,
                               "file": str(shot.relative_to(self.out)), "findings": kept, "excused": excused})
            self.res.check(f"{area}.page:{who}:{width}:{address}", area, f"{call} at {width} px",
                           "document < 400, no 5xx, no console error, no page-check finding",
                           "; ".join(kept)[:600] or f"clean (h1 {h1!r}{', ' + str(len(excused)) + ' excused' if excused else ''})",
                           not kept)

    def dom(self, page, *, portal: bool, viewer: bool) -> list[str]:
        try:
            if page.evaluate("() => !window.__jcSmoke"):
                page.add_script_tag(content=self.script)
            blank = page.evaluate("() => window.__jcSmoke.blankDocument()")
            if portal and blank:
                return ["document: blank (no Portal root, lang or title)"]
            if not portal and page.evaluate("() => !!document.getElementById('root')"):
                # A stopped module's path falls through to the Portal's page fallback.
                return ["document: the Portal answered, not the module this address belongs to"]
            out = page.evaluate("o => window.__jcSmoke.pageFindings(o)", {"namespaces": self.namespaces, "layout": True})
            out += page.evaluate(checks.TABLE_URN)
            if viewer:
                out += page.evaluate("() => window.__jcSmoke.viewerWriteFindings(true)")
            if not portal:
                # Another module's page owes none of the Portal's own rules (header line, row links, Portal
                # i18n keys, UI-16 field hints); it still owes one h1, its words and its width.
                out = [f for f in out if not f.startswith(("purpose-line:", "row-link:", "raw-i18n-key:", "field-hint:"))]
            return out
        except Exception as e:  # noqa: BLE001
            return [f"dom: {type(e).__name__}: {str(e).splitlines()[0][:160]}"]

    def h1(self, page) -> str:
        try:
            loc = page.locator("h1:visible")
            return loc.first.inner_text(timeout=1000).strip()[:120] if loc.count() else ""
        except Exception:  # noqa: BLE001
            return ""

    def screenshot(self, page, who: str, width: int, address: str) -> Path:
        name = re.sub(r"[^A-Za-z0-9]+", "-", address).strip("-") or "root"
        f = self.out / "shots" / who / str(width) / f"{name[:120]}.png"
        f.parent.mkdir(parents=True, exist_ok=True)
        try:
            full = min(int(page.evaluate("() => document.documentElement.scrollHeight")), 6000)
            vp = page.viewport_size
            page.set_viewport_size({"width": vp["width"], "height": max(full, vp["height"])})
            page.screenshot(path=str(f))
            page.set_viewport_size(vp)
        except Exception:  # noqa: BLE001
            try:
                page.screenshot(path=str(f))
            except Exception:  # noqa: BLE001
                pass
        return f


def admin_only(w: Walker, page, who: str):
    """PF-61: the cross-project Endpoints and /api/v1/endpoints reach no one but an administrator."""
    r = page.request.get(w.base + "/api/v1/endpoints")
    want = 200 if who == "admin" else 404
    w.res.check(f"portal.admin-only:{who}", "portal", f"GET /api/v1/endpoints as {who}", str(want),
                str(r.status), r.status == want)


def walk_persona(w: Walker, p: dict, password: str | None):
    who, project = p["name"], p["project"]
    if password is None:
        w.res.skip(f"portal.signin:{who}", "portal", f"POST {LOGIN} as smoke-ui-{who}", "signed in",
                   p.get("skip", "no person"))
        return
    ctx, page = w.context(who)
    try:
        ok, said = w.sign_in(page, who, f"smoke-ui-{who}", password)
        w.res.check(f"portal.signin:{who}", "portal", f"Portal login as smoke-ui-{who} ({p['why']})",
                    "/api/v1/auth/me 200", said, ok)
        if not ok:
            return
        if not project:
            w.res.skip(f"portal.walk:{who}", "portal", "walk", "pages", "no project in the export or --project")
            return
        visits = walk.visits(page, w.base, project, who)
        visits = [v for v in visits if not w.a.only or re.search(w.a.only, v["address"])]
        w.res.check(f"portal.routes:{who}", "portal", f"gather routes of /projects/{project} as {who}",
                    "> 40 addresses" if not w.a.only else "> 0", f"{len(visits)} addresses",
                    len(visits) > (0 if w.a.only else 40))
        viewer = who != "admin"
        for v in visits:
            own = v["route"] in ("/connectors", "/ai-connectors")
            w.judge(page, who, "portal", v["route"], v["address"], portal=True, viewer=viewer and not own)
        admin_only(w, page, who)
        if who == "admin":
            dashboards(w, page, who, project)
            other_uis(w, page, who)
    finally:
        ctx.close()


def dashboards(w: Walker, page, who: str, project: str):
    """The daiya dashboards the Portal lists for the project, against the export's files."""
    want = [f.name for f in sorted((w.a.export / "grafana" / "dashboards").glob(f"org*-project-{project}.json"))] \
        if w.a.export else []
    r = page.request.get(f"{w.base}/api/v1/projects/{project}/grafana/dashboards")
    try:
        body = r.json()
    except Exception:  # noqa: BLE001
        body = None
    items = body.get("items", body) if isinstance(body, dict) else body
    items = items if isinstance(items, list) else []
    w.res.check(f"daiya.list:{project}", "daiya", f"GET /api/v1/projects/{project}/grafana/dashboards as {who}",
                f"200 and >= 1 dashboard (export: {len(want)} file(s) of {project})" if want else "200",
                f"{r.status}, {len(items)} dashboard(s)", r.status == 200 and (len(items) > 0 or not want))
    uids = [str(d.get("uid")) for d in items if isinstance(d, dict) and d.get("uid")]
    for uid in uids[:10]:
        addr = f"/projects/{project}/grafana?dashboard={uid}"
        w.judge(page, who, "daiya", "/projects/$project/grafana", addr, portal=True, viewer=False)
        w.judge(page, who, "daiya", "/dashboards/d/$uid", f"/dashboards/d/{uid}", portal=False, viewer=False)


def other_uis(w: Walker, page, who: str):
    """dataroom and shako, as the signed-in admin's browser (dataroom also anonymously, below)."""
    st = page.request.get(f"{w.base}/git/")
    w.res.check(f"shako.browser:{who}", "shako", f"GET /git/ in a signed-in browser as {who}",
                "200 (a signed-in person reads the repositories)", str(st.status), st.status == 200,
                "" if st.status == 200 else "shako web authenticates a bearer token only: a browser person "
                "signed in to the Portal meets 404 (crates/shako/web web_repo_list)")


def dataroom(w: Walker):
    ctx, page = w.context("anonymous")
    try:
        addrs = ["/opendata/", "/opendata/dataset"]
        for list_page, pattern in (("/opendata/dataset", r"^/opendata/dataset/[^/?#]+$"),
                                   ("/opendata/organization", r"^/opendata/organization/[^/?#]+$")):
            found = walk.first_link(page, w.base, list_page, re.compile(pattern), main_only=False)
            if found:
                addrs.append(found)
            else:
                w.res.add(f"dataroom.item:{list_page}", "dataroom", f"GET {list_page} (first item link)",
                          "a link to one item", "none", FAIL if list_page.endswith("dataset") and w.datasets() else SKIP,
                          "the catalogue lists nothing to open")
        for a in addrs:
            w.judge(page, "anonymous", "dataroom", a, a, portal=False, viewer=False)
        dataset = next((a for a in addrs if a.startswith("/opendata/dataset/")), None)
        if dataset:
            viz_sandbox(w, ctx, page, dataset)
    finally:
        ctx.close()


SANDBOX_PAGES = ("/viz/explore.html", "/viz/embed.html")


def frame_csp_rows(w: Walker, page, shared: bool):
    """A direct GET of each explorer page: a CSP `sandbox` without allow-same-origin when viz shares
    its origin with the Portal (the boundary is the header, not the frame's attribute)."""
    for path in SANDBOX_PAGES:
        r = page.request.get(w.base + path)
        csp = r.headers.get("content-security-policy", "")
        sandboxed = "sandbox allow-scripts" in csp and "allow-same-origin" not in csp
        w.res.check(f"viz.csp:{path}", "viz", f"GET {path}",
                    "CSP sandbox without allow-same-origin" if shared else "CSP without a sandbox (own origin)",
                    f"{r.status} {csp[-120:]}", r.status == 200 and sandboxed == shared)


def viz_sandbox(w: Walker, ctx, page, dataset: str):
    """The explorer frame on one dataset page, with a cookie of the host set: its scripts run in an
    opaque origin that reads no cookie, and the grid still draws rows."""
    page.goto(w.base + dataset, wait_until="load", timeout=60000)
    frame_el = page.locator("iframe.jc-explorer-frame")
    if not frame_el.count():
        w.res.skip("viz.grid", "viz", f"GET {dataset}", "the explorer frame", "the dataset page shows no live data frame")
        return
    src = frame_el.first.get_attribute("src") or ""
    shared = src.startswith((w.base + "/", "/"))
    frame_csp_rows(w, page, shared)
    ctx.add_cookies([{"name": "jc_smoke_probe", "value": "secret", "url": w.base + "/"}])
    page.goto(w.base + dataset, wait_until="load", timeout=60000)
    frame = None
    for _ in range(60):
        frame = next((f for f in page.frames if "/explore.html" in f.url), None)
        if frame:
            break
        page.wait_for_timeout(500)
    if not frame:
        w.res.check("viz.grid", "viz", f"GET {dataset}", "the explorer frame loads", "no frame", False)
        return
    seen = frame.evaluate("""() => {
        let cookie;
        try { cookie = document.cookie; } catch (e) { cookie = "refused: " + e.name; }
        return { origin: self.origin, cookie: cookie };
    }""")
    opaque = seen["origin"] == "null" and "jc_smoke_probe" not in seen["cookie"]
    w.res.check("viz.opaque", "viz", f"the explorer frame on {dataset}, a host cookie set",
                "origin null, no cookie readable" if shared else "the viz origin (own host)",
                f"origin {seen['origin']}, cookie: {seen['cookie'][:60]}", opaque if shared else True)
    try:
        frame.wait_for_selector("table tbody tr", timeout=45000)
        rows = frame.locator("table tbody tr").count()
    except Exception as e:  # noqa: BLE001 - the row says what the frame shows
        status = frame.locator("#status").text_content() or ""
        rows, why = 0, f"{type(e).__name__}; status: {status[:120]}"
    w.res.check("viz.grid", "viz", f"the explorer grid on {dataset}", "rows drawn",
                f"{rows} rows" if rows else why, rows > 0)


# The browser person of shako's own web pages: a bearer token of audience shako. Its name is in
# every check id, so it must not read as `token:<value>` to the logs suite's scan.
SHAKO_PERSONA = "shako-bearer"


def shako(w: Walker):
    token = read_token(w.a.shako_token_file)
    if not token:
        w.res.skip("shako.web", "shako", "GET /git/ with a bearer token", "repository list",
                   "no --shako-token-file (a token of audience shako)")
        return
    ctx, page = w.context(SHAKO_PERSONA, {"Authorization": f"Bearer {token}"})
    try:
        w.judge(page, SHAKO_PERSONA, "shako", "/git/", "/git/", portal=False, viewer=False)
        repo = walk.first_link(page, w.base, "/git/", re.compile(r"^/git/[^/]+/[^/?#]+$"), main_only=False)
        if repo:
            w.judge(page, SHAKO_PERSONA, "shako", "/git/$owner/$repo", repo, portal=False, viewer=False)
        else:
            w.res.add("shako.repo", "shako", "GET /git/ (first repository link)", "a repository", "none",
                      FAIL if w.a.export else SKIP, "the list showed no repository")
    finally:
        ctx.close()


def fold_judge(res: Results, shots: list[dict], judge_file: str | None):
    pages = sorted({(s["page"], s["width"]) for s in shots})
    scored = {}
    if judge_file:
        for row in json.loads(Path(judge_file).read_text()):
            scored[(row.get("page"), int(row.get("width", 0)))] = row
    for page, width in pages:
        row = scored.get((page, width))
        rid, call = f"judge:{width}:{page}", f"UI judge of {page} at {width} px"
        expected = "every score >= 4, no high finding"
        if not row:
            res.skip(rid, "judge", call, expected, "not scored (no --judge-file row)" if judge_file else "no --judge-file")
            continue
        scores = row.get("scores") or {}
        high = [f.get("text", "") for f in row.get("findings") or [] if f.get("severity") == "high"]
        low = {k: v for k, v in scores.items() if isinstance(v, (int, float)) and v < 4}
        actual = ", ".join(f"{k} {v}" for k, v in scores.items())
        if high:
            actual += "; high: " + " | ".join(high)
        res.check(rid, "judge", call, expected, actual[:600], bool(scores) and not low and not high)


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="run ui", description=__doc__.splitlines()[0])
    p.add_argument("--base-url", required=True)
    p.add_argument("--admin-token-file", help="kaisatsu admin token (manage-users in realm jc)")
    p.add_argument("--person-token-file", help="unused: the walk signs in through the browser")
    p.add_argument("--shako-token-file")
    p.add_argument("--export", type=Path)
    p.add_argument("--project")
    p.add_argument("--personas", default="admin,viewer,outsider")
    p.add_argument("--widths", default="1440,400")
    p.add_argument("--only", help="regex over the Portal addresses walked")
    p.add_argument("--judge-file")
    p.add_argument("--out", required=True, type=Path)
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    a.widths = [int(x) for x in a.widths.split(",") if x]
    a.out.mkdir(parents=True, exist_ok=True)
    res = Results("ui", a.base_url)
    h = Http(a.base_url)
    plan = [x for x in ps.plan(a.export, a.project) if x["name"] in a.personas.split(",")]
    passwords: dict[str, str | None] = {}
    token = read_token(a.admin_token_file)
    admin = ps.Admin(h, token) if token else None
    for x in plan:
        if not admin:
            x["skip"] = "no --admin-token-file: no test person can be made"
            passwords[x["name"]] = None
            continue
        try:
            passwords[x["name"]] = admin.person(f"smoke-ui-{x['name']}", x["groups"])
            res.check(f"persona:{x['name']}", "realm", f"make smoke-ui-{x['name']} in realm jc, groups {x['groups']}",
                      "created", "created", True)
        except RuntimeError as e:
            x["skip"] = f"person not made: {e}"
            passwords[x["name"]] = None
            res.check(f"persona:{x['name']}", "realm", f"make smoke-ui-{x['name']} in realm jc", "created", str(e), False)
    if not any(x["name"] == "viewer" for x in plan) and "viewer" in a.personas:
        res.skip("persona:viewer", "realm", "viewer persona", "a provider-group viewer RoleBinding",
                 "the export names no `role: viewer` RoleBinding with a provider group" if a.export else "no --export")

    from playwright.sync_api import sync_playwright

    started = time.time()
    with sync_playwright() as pw:
        browser = pw.chromium.launch(args=["--disable-dev-shm-usage", "--disable-gpu"])
        w = Walker(a, browser, res, a.out)
        w.datasets = lambda: bool(list(a.export.glob("gitea/files/*/*/ckan/*.yaml"))) if a.export else False
        for x in plan:
            walk_persona(w, x, passwords.get(x["name"]))
        dataroom(w)
        shako(w)
        browser.close()
    (a.out / "shots.json").write_text(json.dumps(w.shots, indent=1, ensure_ascii=False))
    fold_judge(res, w.shots, a.judge_file)
    res.write(a.out, {"personas": ", ".join(f"{x['name']} ({x['why']}, project {x['project']})" for x in plan),
                      "widths": ", ".join(map(str, a.widths)), "walk seconds": round(time.time() - started),
                      "shots": f"{len(w.shots)} in shots.json (judge input, JUDGE.md)"})
    t = res.totals()
    print(json.dumps({"out": str(a.out), **t}))
    return 1 if t[FAIL] else 0


if __name__ == "__main__":
    sys.exit(main())
