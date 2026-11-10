"""Playwright (playwright-py from /opt/jc-py) shots of the Portal's dashboard pages and the Apps'
main pages, signed in with the suite's session cookies. Each shot is full height (the viewport grows
to the page's scrollHeight, so a sticky sidebar is not cut)."""

from __future__ import annotations

import contextlib
import ipaddress
import re
import urllib.parse
from pathlib import Path

WIDTHS = (1440, 400)


def app_host(base: str, name: str) -> str:
    """The App's own origin, `{name}.apps.{host}` (AP-133); Chromium resolves `*.localhost` to loopback.
    An IP literal has no subdomains: a base on 127.0.0.1 has its App hosts under localhost, as the
    rehearsal kit mounts them (`*.apps.localhost:<port>`, deploy/rehearsal/up.sh)."""
    u = urllib.parse.urlparse(base)
    host = u.hostname or ""
    try:
        ipaddress.ip_address(host)
        host = "localhost"
    except ValueError:
        pass
    port = f":{u.port}" if u.port else ""
    return f"{u.scheme}://{name}.apps.{host}{port}"


def edge_rewrite(base: str, name: str, url: str) -> str | None:
    """Where the edge sends a request on the App's host, by the two rules the Portal's edge file
    gives APISIX (crates/portal/src/reconciler/edge_file.rs, static App): `/api/endpoint/*` to the
    gateway with any `/apps/{name}` prefix stripped, everything else to `/apps/{name}/`. The
    rehearsal runs no APISIX, so the suite stands in for it; the request keeps the App's host as
    its `Host` (Browser.app), where jc's App host layer takes either path, since the main host
    answers `/apps/{name}/` with prod's 308 to that host. `None` for a request off that host."""
    u = urllib.parse.urlparse(url)
    if f"{u.scheme}://{u.netloc}" != app_host(base, name):
        return None
    path = u.path + (f"?{u.query}" if u.query else "")
    n = re.escape(name)
    if m := re.match(rf"^(?:/apps/{n})?(/api/endpoint/.*)$", path):
        return base.rstrip("/") + m.group(1)
    return base.rstrip("/") + re.sub(rf"^/(?:apps/{n}/)?(.*)$", rf"/apps/{name}/\1", path)


class Browser:
    def __init__(self, base: str, jar: dict, timeout_ms: int = 45000):
        from playwright.sync_api import sync_playwright

        self.base, self.timeout = base.rstrip("/"), timeout_ms
        self._pw = sync_playwright().start()
        self._browser = self._pw.chromium.launch(args=["--disable-dev-shm-usage", "--disable-gpu"])
        host = urllib.parse.urlparse(self.base).hostname
        self.cookies = [{"name": n, "value": v, "domain": host, "path": p, "secure": n.startswith(("__Host-", "__Secure-")),
                         "httpOnly": True, "sameSite": "Lax"} for n, (v, p) in jar.items()]

    def close(self):
        self._browser.close()
        self._pw.stop()

    def _page(self, width: int, portal: bool = True):
        """`portal`: with the suite's Portal session cookies. An App's page gets none: its host is
        another origin, and route.fetch would send the Portal's CSRF cookie beside the App's."""
        ctx = self._browser.new_context(viewport={"width": width, "height": 900}, locale="en-GB")
        if portal:
            ctx.add_cookies(self.cookies)
        page = ctx.new_page()
        page.set_default_timeout(self.timeout)
        return ctx, page

    def _settle(self, page):
        from playwright.sync_api import TimeoutError as PlaywrightTimeout

        with contextlib.suppress(PlaywrightTimeout):  # a page that keeps polling still gets its shot
            page.wait_for_load_state("networkidle", timeout=self.timeout)
        page.wait_for_timeout(1500)

    def _shoot(self, page, width: int, out: Path):
        height = page.evaluate("() => Math.min(document.documentElement.scrollHeight, 12000)")
        page.set_viewport_size({"width": width, "height": max(900, int(height))})
        page.wait_for_timeout(400)
        out.parent.mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(out))

    def dashboard(self, project: str, uid: str, out_dir: Path) -> list[dict]:
        rows = []
        path = f"/projects/{urllib.parse.quote(project)}/grafana/dashboards/{urllib.parse.quote(uid)}"
        for width in WIDTHS:
            ctx, page = self._page(width)
            panel_answers = []
            page.on("response", lambda r, seen=panel_answers: seen.append(r.status) if "/panels/" in r.url else None)
            row = {"width": width, "url": path}
            try:
                resp = page.goto(self.base + path)
                row["status"] = resp.status if resp else 0
                self._settle(page)
                row["final"] = urllib.parse.urlparse(page.url).path
                row["h1"] = (page.locator("h1").first.inner_text(timeout=5000) or "").strip()
                row["alerts"] = [t.strip()[:160] for t in page.get_by_role("alert").all_inner_texts() if t.strip()]
                row["panelAnswers"] = panel_answers
                shot = out_dir / f"{project}--{uid}--{width}.png"
                self._shoot(page, width, shot)
                row["shot"] = str(shot)
            except Exception as e:  # noqa: BLE001 - a failed shot is a failed check, said in the row
                row["error"] = str(e).splitlines()[0][:200]
            finally:
                ctx.close()
            rows.append(row)
        return rows

    def app(self, name: str, out_dir: Path, token: str | None = None,
            login: tuple[str, str] | None = None) -> list[dict]:
        """The App on its own host, as prod serves it. `token`: a person's token of the App's own
        client, sent as the edge sends it on prod. `login` (username, password): jc's App host runs
        the App client's own code flow, so a role-gated App first shows the realm's sign-in form;
        the person signs in there as on prod's edge login, and only what loads after counts."""
        rows, path = [], app_host(self.base, name) + "/"
        for width in WIDTHS:
            ctx, page = self._page(width, portal=False)

            def edge(route, request, base=self.base):
                url = edge_rewrite(base, name, request.url)
                # The edge's login hands the person's token on as the edge file says: the
                # gateway's bearer on the endpoint route, X-Access-Token on the App's own.
                headers = {k: v for k, v in request.headers.items() if k.lower() not in ("authorization", "host")}
                headers["host"] = urllib.parse.urlparse(request.url).netloc
                if token and "/api/endpoint/" in url:
                    headers["authorization"] = f"Bearer {token}"
                elif token:
                    headers["x-access-token"] = token
                # fetched and fulfilled under the App's URL: a continue_ to another URL would
                # move the document off its origin, and its own CSP `'self'` refuse its assets
                route.fulfill(response=route.fetch(url=url, headers=headers, max_redirects=0))

            ctx.route(lambda url, base=self.base: edge_rewrite(base, name, url) is not None, edge)
            data, bad, console = [], [], []
            page.on("console", lambda m, errors=console: errors.append(m.text[:160]) if m.type == "error" else None)

            def seen(r, data=data, bad=bad):
                if "/api/" not in r.url:
                    return
                (data if 200 <= r.status < 300 else bad).append(f"{r.status} {urllib.parse.urlparse(r.url).path[:120]}")

            page.on("response", seen)
            row = {"width": width, "url": path}
            try:
                resp = page.goto(path)
                row["status"] = resp.status if resp else 0
                self._settle(page)
                if login and page.locator("input[type=password]").count():
                    page.locator("input[name=username]").fill(login[0])
                    page.locator("input[type=password]").fill(login[1])
                    del data[:], bad[:]
                    with page.expect_navigation():
                        page.locator("input[type=password]").press("Enter")
                    self._settle(page)
                    row["signedIn"] = login[0]
                row["title"] = page.title()
                row.update(data=data[:20], dataCount=len(data), bad=bad[:20], console=console[:10])
                shot = out_dir / f"app--{name}--{width}.png"
                self._shoot(page, width, shot)
                row["shot"] = str(shot)
            except Exception as e:  # noqa: BLE001
                row["error"] = str(e).splitlines()[0][:200]
            finally:
                # a page still polling has routes in flight: they end quietly, not in the next shot
                ctx.unroute_all(behavior="ignoreErrors")
                ctx.close()
            rows.append(row)
        return rows
