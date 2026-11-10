"""The addresses the live walker opens, for one project: a port of
crates/portal/ui/e2e/live/walk-routes.ts `visits` (every route of the router with one real item
for each name it takes, and every create and edit form at its own address). Keep the lists in
step with that file; tests/test_ui.py compares them."""

from __future__ import annotations

import re
import urllib.parse

from common import browser_get

FORMS = ["spaces", "endpoints", "pipelines", "policies", "subscriptions", "csrs", "datasources", "syncsources"]
SECTIONS = ["spaces", "endpoints", "subscriptions", "datasources", "pipelines", "dashboards", "apps", "syncsources",
            "policies", "csrs"]
ORGANIZATION_TABS = ["settings", "people", "members", "roles", "groups", "service-accounts", "projects"]
SETTINGS_TABS = ["general", "members", "roles", "service-accounts", "access", "danger"]
PROJECT_PAGES = ["home", "inbox", "workspaces", "activity", "grafana", "health", "approvals", "access", "settings",
                 "models", "explore", "ckan", "mcp", "federation", "import", "assistant", "shared", "knowledge",
                 "knowledge/conversations"]
TOP = ["/catalogue", "/glossary", "/connectors", "/ai-connectors", "/settings", "/playground"]


def first_link(page, base: str, list_page: str, pattern: re.Pattern, main_only: bool = True, tries: int = 15):
    """The first link on a list page that opens one of its items, as a person would find it."""
    try:
        page.goto(f"{base}{list_page}", wait_until="load", timeout=60000)
    except Exception:  # noqa: BLE001 - the list page's own row reports a failed load
        return None
    sel = "main a[href]" if main_only else "a[href]"
    for _ in range(tries):
        hrefs = page.locator(sel).evaluate_all("ls => ls.map(l => l.getAttribute('href') || '')")
        for href in hrefs:
            path = href.split("?")[0].split("#")[0]
            if path.startswith(base):
                path = path[len(base):]
            if pattern.search(path):
                return path
        page.wait_for_timeout(1000)
    return None


def first_name(page, base: str, project: str, section: str):
    r = browser_get(page, f"{base}/api/v1/projects/{project}/{section}")
    if not r.ok:
        return None
    try:
        items = (r.json() or {}).get("items") or []
    except Exception:  # noqa: BLE001
        return None
    return ((items[0] if items else {}).get("metadata") or {}).get("name")


def visits(page, base: str, project: str, who: str) -> list[dict]:
    P = f"/projects/{project}"
    found = [{"route": "/", "address": "/"}, {"route": "/endpoints", "address": "/endpoints"},
             {"route": "/organization", "address": "/organization"}]
    found += [{"route": r, "address": r} for r in TOP]
    found += [{"route": "/organization/$tab", "address": f"/organization/{t}"}
              for t in ORGANIZATION_TABS + (["endpoints"] if who == "admin" else [])]
    found += [{"route": "/organization/$tab/$", "address": "/organization/people/new"},
              {"route": "/organization/$tab/$", "address": "/organization/groups/new"}]
    found += [{"route": f"/projects/$project/{x}", "address": f"{P}/{x}"} for x in PROJECT_PAGES]
    for x in ("workspaces/new", "secrets/new", "spaces/complete", "spaces/upload"):
        found.append({"route": f"/projects/$project/{x}", "address": f"{P}/{x}"})
    found += [{"route": "/projects/$project/settings/$tab", "address": f"{P}/settings/{t}"} for t in SETTINGS_TABS]
    found += [{"route": "/projects/$project/settings/$tab/$", "address": f"{P}/settings/{t}/new"}
              for t in ("members", "roles", "service-accounts")]
    found += [{"route": "/projects/$project/$plural", "address": f"{P}/{s}"} for s in SECTIONS]
    found += [{"route": "/projects/$project/$plural/new", "address": f"{P}/{s}/new"} for s in FORMS]
    q = re.escape(P)
    items = [
        ("/projects/$project/$plural/$name", f"{P}/spaces", rf"^{q}/spaces/(?!complete$)[^/]+$"),
        ("/projects/$project/$plural/$name", f"{P}/endpoints", rf"^{q}/endpoints/[^/]+$"),
        ("/projects/$project/$plural/$name", f"{P}/apps", rf"^{q}/apps/[^/]+$"),
        ("/projects/$project/knowledge/$source", f"{P}/knowledge", rf"^{q}/knowledge/(?!conversations$)[^/]+$"),
        ("/projects/$project/models/$name", f"{P}/models", rf"^{q}/models/[^/]+$"),
        ("/projects/$project/approvals/$id", f"{P}/approvals", rf"^{q}/approvals/[^/]+$"),
        ("/projects/$project/workspaces/$name/compare", f"{P}/workspaces", rf"^{q}/workspaces/[^/]+/compare$"),
        ("/organization/$tab/$", "/organization/people", r"^/organization/people/(?!new$)[^/]+$"),
        ("/organization/$tab/$", "/organization/groups", r"^/organization/groups/(?!new$)[^/]+$"),
        ("/catalogue/$name", "/catalogue", r"^/catalogue/[^/]+$"),
    ]
    for route, list_page, pattern in items:
        address = first_link(page, base, list_page, re.compile(pattern))
        if address:
            found.append({"route": route, "address": address})
    copy = next((v["address"][: -len("/compare")] for v in found if v["route"].endswith("/compare")), None)
    if copy:
        found.append({"route": "/projects/$project/workspaces/$name/try-it", "address": f"{copy}/try-it"})
        found.append({"route": "/projects/$project/workspaces/$name/bring-back", "address": f"{copy}/bring-back"})
    server = first_name(page, base, project, "mcpservers")
    if server:
        found.append({"route": "/connect/$project/$name", "address": f"/connect/{project}/{urllib.parse.quote(server)}"})
    for section in FORMS:
        name = first_name(page, base, project, section)
        if name:
            found.append({"route": "/projects/$project/$plural/$name/edit",
                          "address": f"{P}/{section}/{urllib.parse.quote(name)}/edit"})
    # Each address once, with ?lang=en as the walker opens it.
    seen, out = set(), []
    for v in found:
        if v["address"] not in seen:
            seen.add(v["address"])
            sep = "&" if "?" in v["address"] else "?"
            out.append({"route": v["route"], "address": f"{v['address']}{sep}lang=en"})
    return out
