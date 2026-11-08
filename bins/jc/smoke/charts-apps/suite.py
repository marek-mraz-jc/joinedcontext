"""Smoke suite `charts-apps`: dashboards in daiya and the Portal, and the exported Apps.

    bins/jc/smoke/run charts-apps --base-url http://localhost:42101 --export <bundle> \
        --state <rehearsal state dir> [--dashboards <rehearsal.py dashboards dir>]... --out <dir>

Dashboards (the export's grafana/dashboards and every --dashboards dir, once per project and uid):
  daiya    the organization, the dashboard by uid, and each panel's saved queries run through
           daiya's render route for the rehearsal's window (24 h back from now, not before the
           export's cut-off); Prometheus panels skip when daiya holds no Prometheus datasource.
  portal   the project's dashboard list, the dashboard, each panel through the Portal (jc-charts);
           organization 1 (platform models) is the Portal's project `org`.
  shots    the Portal page at 1440 and 400 px (Playwright), with its alerts read.
  Whether a panel should have rows comes from the export: entities of its type in the
  Endpoint's tenant, or for a temporal query an instance observed inside the window.
Apps (every App manifest of the export's repositories, every joinedcontext-apps repository):
  the Portal reads the App, shako holds its repository, /apps/{name}/ serves it and its main page
  reads data. jc has no App build runner (owner decision open): an App that is not served skips
  naming it.

Credentials come from the rehearsal's state dir (0600 files written by up.sh): dashboards-password
(daiya Basic), portal-admin-client-secret (makes the suite's person), shako-token. --admin-token-file
takes a portal-reconciler token instead of the secret. No secret value is written anywhere."""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import json
import re
import sys
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent / "endpoints"))

import export as ex
from common import FAIL, PASS, SKIP, Results, loopback_only, read_token

import inventory as inv
from session import Session, admin_token, make_person, sign_in

NO_RUNNER = ("jc has no App build runner (App builds runner is an open owner decision: shako-native "
             "runner, supervisor 2026-11-08)")


def frame_rows(frames) -> int:
    n = 0
    for f in frames or []:
        if not isinstance(f, dict):
            continue
        fields = f.get("fields") or (f.get("schema") or {}).get("fields") or []
        values = [x.get("values") for x in fields if isinstance(x, dict) and isinstance(x.get("values"), list)]
        if not values:
            values = (f.get("data") or {}).get("values") or []
        n += max((len(v) for v in values if isinstance(v, list)), default=0)
    return n


def daiya_frames(answer) -> tuple[list, str]:
    """(frames, error) of daiya's GET /api/dashboards/{uid}/render answer."""
    if not isinstance(answer, dict):
        return [], f"not JSON: {str(answer)[:120]}"
    if answer.get("error") or answer.get("message"):
        return [], str(answer.get("error") or answer.get("message"))[:200]
    frames = []
    for s in answer.get("frameSets") or []:
        if s.get("error"):
            return [], str(s["error"])[:200]
        frames += s.get("frames") or []
    return frames, ""


def judge(status: int, frames, error: str, expect: str, why: str) -> tuple[str, str, str]:
    """(result, actual, reason) of one panel answer against what the export says."""
    rows = frame_rows(frames)
    actual = f"{status}, {len(frames or [])} frames, {rows} rows" + (f", error: {error}" if error else "")
    if status != 200 or error:
        return FAIL, actual, ""
    if rows:
        return PASS, actual, "" if expect != "empty" else f"rows although the export says: {why}"
    if expect == "data":
        return FAIL, actual, f"no rows; the export says: {why}"
    if expect == "empty":
        return PASS, actual, f"empty, as the export: {why}"
    return SKIP, actual, f"no rows and nothing to compare with: {why}"


class Daiya:
    def __init__(self, s: Session, password: str | None):
        self.s = s
        self.auth = {"Authorization": "Basic " + base64.b64encode(f"portal:{password}".encode()).decode()} if password else None
        self.orgs: dict = {}
        self.ds: dict = {}

    def org(self, project: str):
        if project == inv.ORG_PROJECT:
            return 1, "organization 1"
        if project not in self.orgs:
            st, o = self.s.json("GET", f"/dashboards/api/orgs/name/{urllib.parse.quote(project)}", headers=self.auth)
            self.orgs[project] = (o.get("id"), "") if st == 200 and isinstance(o, dict) else (None, f"{st} {str(o)[:120]}")
        return self.orgs[project]

    def hdrs(self, org: int) -> dict:
        return {**self.auth, "X-Grafana-Org-Id": str(org)}

    def datasources(self, org: int) -> dict:
        if org not in self.ds:
            st, body = self.s.json("GET", "/dashboards/api/datasources", headers=self.hdrs(org))
            self.ds[org] = {d.get("uid"): d for d in body} if st == 200 and isinstance(body, list) else {}
        return self.ds[org]


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--base-url", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--export", required=True, help="the export bundle dir")
    p.add_argument("--dashboards", action="append", default=[],
                   help="a rehearsal.py dashboards dir (the deployment's Grafana files); repeatable")
    p.add_argument("--state", help="the rehearsal's state dir (dashboards-password, portal-admin-client-secret, shako-token)")
    p.add_argument("--admin-token-file", help="a portal-reconciler token (else from --state)")
    p.add_argument("--person-token-file", help="unused by this suite (accepted for the common runner)")
    p.add_argument("--cache", help="where the export index is kept (default <out>/../.cache)")
    p.add_argument("--only", help="regex over project, dashboard uid or App name")
    p.add_argument("--no-shots", dest="shots", action="store_false")
    p.add_argument("--no-apps", dest="apps", action="store_false")
    p.add_argument("--timeout", type=float, default=60)
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    export, out = Path(a.export), Path(a.out)
    state = Path(a.state) if a.state else None
    only = re.compile(a.only) if a.only else None
    res = Results("charts-apps", a.base_url)
    s = Session(a.base_url, a.timeout)

    def secret(name):
        f = state / name if state else None
        return f.read_text().strip() if f and f.exists() else None

    manifest = json.loads((export / "manifest.json").read_text())
    now = dt.datetime.now(dt.UTC)
    cutoff = manifest.get("temporalCutoff")
    since = max(now - dt.timedelta(hours=24),
                ex.parse_time(cutoff) if cutoff else now - dt.timedelta(hours=24))
    window = {"from": str(int(since.timestamp() * 1000)), "to": str(int(now.timestamp() * 1000))}
    meta = {"export": str(export), "exportedAt": manifest.get("exportedAt"), "window":
            f"{since.isoformat(timespec='seconds')} .. {now.isoformat(timespec='seconds')}"}

    # ---- the person and the Portal session
    signed = False
    try:
        tok = read_token(a.admin_token_file) or (admin_token(s, secret("portal-admin-client-secret"))
                                                  if secret("portal-admin-client-secret") else None)
        if not tok:
            res.skip("session", "portal", "sign in", "a signed-in platform admin",
                     "no --admin-token-file and no portal-admin-client-secret in --state")
        else:
            ok, detail = sign_in(s, make_person(s, tok))
            res.check("session", "portal", "GET /api/v1/auth/login (code flow)", "signed in", detail, ok)
            signed = ok
    except RuntimeError as e:
        res.check("session", "portal", "sign in as smoke-charts-apps", "signed in", str(e)[:200], False)

    dirs = [export / "grafana" / "dashboards", *map(Path, a.dashboards)]
    dashboards, file_ds = inv.load_dashboards([d for d in dirs if d.exists()])
    endpoints = ex.load_endpoints(export / "gitea" / "files")
    slug_tenant = {e.slug: e.tenant for e in endpoints}
    index = ex.load_index(export, endpoints, Path(a.cache) if a.cache else out.parent / ".cache", log=lambda m: print(m, file=sys.stderr))
    types = {t.get("entityType") for d in dashboards for pn in d.panels for t in pn.targets if t.get("entityType")}
    temporal = inv.temporal_index(export, types)
    daiya = Daiya(s, secret("dashboards-password"))
    browser = None
    shots_dir = out / "shots"
    counts = {"dashboards": 0, "panels": 0}

    for d in dashboards:
        if only and not only.search(f"{d.project} {d.uid}"):
            continue
        counts["dashboards"] += 1
        tgt = f"dashboard {d.project}/{d.uid}"
        src = ", ".join(d.sources)
        if d.problem:
            res.check("dash.project", tgt, f"files {src}", "a project for the dashboard", d.problem, False)
            continue
        # -- daiya
        org = None
        if not daiya.auth:
            res.skip("daiya.dashboard", tgt, "GET /dashboards/api/dashboards/uid/{uid}", "200",
                     "no dashboards-password in --state")
        else:
            org, why = daiya.org(d.project)
            res.check("daiya.org", tgt, f"GET /dashboards/api/orgs/name/{d.project}", "the project's organization",
                      f"org {org}" if org else why, bool(org))
        held = None
        if org:
            st, held = s.json("GET", f"/dashboards/api/dashboards/uid/{urllib.parse.quote(d.uid)}", headers=daiya.hdrs(org))
            model = (held or {}).get("dashboard") if st == 200 and isinstance(held, dict) else None
            n_held = len(inv.panels_of(model)) if model else 0
            res.check("daiya.dashboard", tgt, f"GET /dashboards/api/dashboards/uid/{d.uid} (org {org})",
                      f"200 with {len(d.panels)} panels ({src})", f"{st}, {n_held} panels", st == 200 and n_held == len(d.panels))
            held = model
        dsx = daiya.datasources(org) if org else {}
        for pn in d.panels:
            counts["panels"] += 1
            ptgt = f"{tgt}#{pn.id} {pn.title[:50]}"
            if pn.query_free():
                res.skip("panel", ptgt, "-", "-", f"a {pn.type} panel runs no query")
                continue
            dsrc = dsx.get(pn.ds_uid) or {}
            is_prom = pn.ds_type == inv.PROMETHEUS or dsrc.get("type") == inv.PROMETHEUS or pn.ds_uid == inv.PROMETHEUS
            if is_prom and not any(x.get("type") == inv.PROMETHEUS for x in dsx.values()):
                for check_id in ("daiya.panel", "portal.panel"):
                    res.skip(check_id, ptgt, "-", "frames with data",
                             f"no Prometheus in the rehearsal (datasource {pn.ds_uid!r}; daiya org {org} holds none)")
                continue
            slug = inv.urlparse_slug(dsrc.get("url", "")) or next(
                (x.slug for x in file_ds if x.project == d.project and x.uid == pn.ds_uid), "")
            expect, why = inv.expect_panel(pn, slug_tenant.get(slug), index.get("tenants", {}), temporal, since)
            exp_txt = f"200, frames, {'rows' if expect == 'data' else 'no rows' if expect == 'empty' else 'rows or skip'} ({why})"
            if org and held is not None:
                q = urllib.parse.urlencode({"panel": pn.id, **window})
                st, ans = s.json("GET", f"/dashboards/api/dashboards/{urllib.parse.quote(d.uid)}/render?{q}", headers=daiya.hdrs(org))
                frames, err = daiya_frames(ans)
                if st != 200 and not err and isinstance(ans, dict):
                    err = str(ans)[:160]
                r, act, reason = judge(st, frames, err, expect, why)
                res.add("daiya.panel", ptgt, f"GET /dashboards/api/dashboards/{d.uid}/render?panel={pn.id}&from&to (org {org}, datasource {pn.ds_uid})",
                        exp_txt, act, r, reason)
            elif daiya.auth:
                res.add("daiya.panel", ptgt, "render", exp_txt, "", FAIL, "the dashboard is not in daiya")
            # -- the same panel through the Portal
            if not signed:
                res.skip("portal.panel", ptgt, "GET /api/v1/projects/{p}/grafana/dashboards/{uid}/panels/{id}", exp_txt, "not signed in")
                continue
            q = urllib.parse.urlencode(window)
            path = f"/api/v1/projects/{d.project}/grafana/dashboards/{urllib.parse.quote(d.uid)}/panels/{pn.id}?{q}"
            st, ans = s.json("GET", path)
            ans = ans if isinstance(ans, dict) else {"error": str(ans)[:160]}
            err = ans.get("error") or ans.get("accessDenied") or (ans.get("detail") if st != 200 else "") or ""
            r, act, reason = judge(st, ans.get("frames"), str(err)[:200], expect, why)
            res.add("portal.panel", ptgt, f"GET {path.split('?')[0]}", exp_txt, act, r, reason)
        # -- Portal list, page, shots
        if not signed:
            res.skip("portal.dashboard", tgt, "GET /api/v1/projects/{p}/grafana/dashboards", "listed and opened", "not signed in")
            continue
        base = f"/api/v1/projects/{d.project}/grafana/dashboards"
        st, listed = s.json("GET", base)
        items = (listed or {}).get("items", []) if isinstance(listed, dict) else []
        res.check("portal.list", tgt, f"GET {base}", f"ready, {d.uid} listed",
                  f"{st}, ready={(listed or {}).get('ready') if isinstance(listed, dict) else None}, {len(items)} items"
                  + (f", reason: {listed.get('reason')}" if isinstance(listed, dict) and listed.get("reason") else ""),
                  st == 200 and any(i.get("uid") == d.uid for i in items))
        st, view = s.json("GET", f"{base}/{urllib.parse.quote(d.uid)}")
        nview = len(inv.panels_of(view)) if st == 200 and isinstance(view, dict) else 0
        res.check("portal.dashboard", tgt, f"GET {base}/{d.uid}", f"200 with {len(d.panels)} panels",
                  f"{st}, {nview} panels", st == 200 and nview == len(d.panels))
        if a.shots:
            if browser is None:
                from shots import Browser
                browser = Browser(a.base_url, s.jar)
            for row in browser.dashboard(d.project, d.uid, shots_dir):
                bad = [x for x in row.get("panelAnswers", []) if x >= 400]
                ok = (row.get("status") == 200 and not row.get("error") and not row.get("alerts") and not bad
                      and d.title.split(" ")[0].lower() in (row.get("h1") or "").lower())
                res.check(f"shot.{row['width']}", tgt, f"browser {row['url']} at {row['width']} px",
                          f"the page draws '{d.title}', no alert, no failed panel read",
                          f"{row.get('status')}, h1 {row.get('h1')!r}, alerts {row.get('alerts')}, panel reads "
                          f"{len(row.get('panelAnswers', []))} ({len(bad)} failed){', ' + row['error'] if row.get('error') else ''}; "
                          f"shot {Path(row['shot']).relative_to(out) if row.get('shot') else '-'}", ok)

    # ---- Apps
    if a.apps:
        apps = inv.load_apps(export / "gitea" / "files")
        repos = [r for r in json.loads((export / "gitea" / "repos.json").read_text()) if r["owner"] == "joinedcontext-apps"]
        shako = secret("shako-token")
        sauth = {"Authorization": f"token {shako}"} if shako else None
        named = {x.git for x in apps}
        counts.update(apps=len(apps), appRepos=len(repos))
        for r in repos:
            full = f"{r['owner']}/{r['name']}"
            if only and not only.search(full):
                continue
            call = f"GET /git/api/v1/projects/{r['owner']}/repositories/{r['name']}"
            if not sauth:
                res.skip("app.repo", f"repo {full}", call, "200", "no shako-token in --state")
                continue
            st, _ = s.json("GET", call.split(" ", 1)[1], headers=sauth)
            res.check("app.repo", f"repo {full}", call, "200 (imported into shako)",
                      f"{st}{'' if full in named else ', no App manifest names it'}", st == 200)
        for app in apps:
            if only and not only.search(f"{app.project} {app.name}"):
                continue
            tgt = f"app {app.project}/{app.name} ({app.kind})"
            if not signed:
                res.skip("app.portal", tgt, "GET /api/v1/projects/{p}/apps/{name}", "200", "not signed in")
                continue
            path = f"/api/v1/projects/{app.project}/apps/{urllib.parse.quote(app.name)}"
            st, held = s.json("GET", path)
            res.check("app.portal", tgt, f"GET {path}", f"200 (from {app.file})", f"{st}", st == 200)
            if app.git and sauth:
                owner, repo = app.git.split("/", 1)
                call = f"GET /git/api/v1/projects/{owner}/repositories/{repo}"
                st, _ = s.json("GET", call.split(" ", 1)[1], headers=sauth)
                res.check("app.source", tgt, call, "200 (the App's spec.source.git is in shako)", f"{st}", st == 200)
            if app.lifecycle != "published":
                res.skip("app.served", tgt, f"GET /apps/{app.name}/", "-", f"lifecycle {app.lifecycle!r}: not served by design")
                continue
            st, hdrs, _ = s.call("GET", f"/apps/{urllib.parse.quote(app.name)}/")
            served = st == 200 and "html" in hdrs.get("content-type", "")
            bst, build = s.json("GET", f"{path}/build")
            status = (held or {}).get("status") if isinstance(held, dict) else None
            state_txt = (f"/apps/{app.name}/ {st}; build {bst} {json.dumps(build)[:140] if isinstance(build, dict) else ''}; "
                         f"status {json.dumps(status)[:160] if status else 'none'}")
            if served:
                res.check("app.served", tgt, f"GET /apps/{app.name}/", "200 text/html", f"{st} {hdrs.get('content-type')}", True)
            else:
                res.skip("app.served", tgt, f"GET /apps/{app.name}/", "200 text/html (built and served as before)",
                         f"{NO_RUNNER}: {state_txt}")
            if not served:
                res.skip("app.page", tgt, f"browser /apps/{app.name}/", "main page reads data", f"not served: {NO_RUNNER}")
                continue
            if not a.shots:
                res.skip("app.page", tgt, f"browser /apps/{app.name}/", "main page reads data", "--no-shots")
                continue
            if browser is None:
                from shots import Browser
                browser = Browser(a.base_url, s.jar)
            for row in browser.app(f"/apps/{urllib.parse.quote(app.name)}/", app.name, shots_dir):
                ok = row.get("status") == 200 and row.get("dataCount", 0) > 0 and not row.get("bad") and not row.get("error")
                res.check(f"app.page.{row['width']}", tgt, f"browser {row['url']} at {row['width']} px",
                          "200, at least one /api/ read 2xx, none failed",
                          f"{row.get('status')}, {row.get('dataCount', 0)} reads ok, failed {row.get('bad')}, "
                          f"console {row.get('console')}{', ' + row['error'] if row.get('error') else ''}", ok)
    if browser:
        browser.close()
    meta.update(counts)
    res.write(out, meta)
    t = res.totals()
    print(json.dumps({"out": str(out), **t}))
    return 1 if t[FAIL] else 0


if __name__ == "__main__":
    sys.exit(main())
