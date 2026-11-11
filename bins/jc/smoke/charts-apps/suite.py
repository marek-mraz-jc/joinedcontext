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
  the Portal reads the App, shako holds its repository, /apps/{name}/ answers prod's 308 to the
  App's own host {name}.apps.{host}/ and that host (the base URL with its Host header) serves it (the image's
  bundles, and the prod builds deploy/rehearsal/apps.sh publishes into shako) and its main page
  reads data. A public App is opened anonymously; any other on a token of the App's own client
  (app-{name}, the code flow the edge runs on prod) as rehearsal-app-roles, and refused to
  rehearsal-tester: 403 for a roles App, 404 for the others. A wasm App's server half: GET /api/me
  on the App's own host, which the Portal hands to the App's shard (AP-143), answers the App's own
  200 JSON (null anonymously, the caller on a token); signed out, a non-public App's answers 401.
  --no-wasm-host (an installation without a shard): the Portal's designed 503 problem naming the App
  instead, and app.server a named skip.
  Named skips: a wasm App's browser page (the suite's stand-in edge routes no server path; app.server
  checks it), a ui-rust App (kept component, a Kubernetes Service), an App the source never built.

Credentials come from the rehearsal's state dir (0600 files written by up.sh): dashboards-password
(daiya Basic), portal-admin-client-secret (makes the suite's person), shako-token, and realms/*.json
(the App clients' secrets and the rehearsal persons' passwords, each by its $(file:...) reference). --realm-token-file
takes a portal-reconciler token instead of the secret (not the runner's --admin-token-file: that is
jc's admin listener token, which the realm refuses). No secret value is written anywhere."""

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
from common import FAIL, PASS, SKIP, Results, loopback_only, read_token, realm_of

import inventory as inv
from session import (EDGE_CLIENT, HOLDER, PERSON, READER, TESTER, Realm, Session, admin_token, app_token,
                     make_person, sign_in)
from shots import app_host as shots_app_host

APPS_OWNER = "joinedcontext-apps"
# Rows the rehearsal cannot prove, each named with its reason (CUTOVER.md keeps these components
# beside jc; dev-smoke and jc-smoke prove them on dev).
NEVER_BUILT = ("same on prod: never built (status.build unset in the export; prod's host has no build to "
               "serve either)")
KEPT = {"ui-rust": ("kept component: Kubernetes Service (a ui-rust App's server runs as its own Service "
                    "beside jc, stamped by the Portal through the cluster API; the rehearsal has no Kubernetes)")}
NO_SHARD = ("kept component: no wasm-host shard (--no-wasm-host: the rehearsal kit runs no jc-wasm-host and no "
            "apps-db yet); app.server.no-shard asserts the Portal's designed 503 instead")
WASM_PAGE = ("the suite's stand-in edge (shots.edge_rewrite) routes no wasm server path; app.server checks "
             "the server half through the Portal's App host")


def server_judge(status: int, headers: dict, raw: bytes, signed_in: bool) -> tuple[str, bool]:
    """(actual, ok) of a wasm App's GET /api/me on its own host. Every wasm App of the export serves
    /api/me: 200 JSON, null for an anonymous caller, the caller ({id, ...}) on a token. The Portal's
    503 (no shard), a problem+json or any other answer fails."""
    ctype = headers.get("content-type", "")
    actual = f"{status} {ctype or '-'} {raw[:120].decode(errors='replace')!r}"
    if status == 200 and ctype.startswith("text/html"):
        return ("a page, not the App's server: is jc's App host layer on (a *.apps mount, REHEARSAL_APP_HOSTS=on)? "
                + actual), False
    if status != 200 or not ctype.startswith("application/json"):
        return actual, False
    try:
        body = json.loads(raw)
    except ValueError:
        return actual, False
    return actual, (isinstance(body, dict) and bool(body.get("id"))) if signed_in else body is None


def moved_to(base: str, name: str, status: int, headers: dict) -> tuple[str | None, str]:
    """Prod's answer to `/apps/{name}/` on the main host: a 308 to the App's own origin
    `{name}.apps.{host}/` (T-2476, AP-133: the Portal's static_host `to_app_origin` under
    JC_PORTAL_APPS_URL, and APISIX's `app-{name}-moved` route jcctl rendered). (that Location, "")
    when the answer is exactly that; else (None, what came): a Location anywhere else is never
    followed, and a page served on the main host is not what prod answered."""
    want = shots_app_host(base, name) + "/"
    loc = headers.get("location")
    if status == 308 and loc == want:
        return loc, ""
    if status == 308:
        return None, f"308 to {loc!r}, not the App's own origin {want}: not followed"
    actual = f"{status} {headers.get('content-type')}"
    if status == 200:
        actual += f": served on the main host, where prod answered 308 to {want}: is jc's App host layer on (REHEARSAL_APP_HOSTS=on)?"
    return None, actual


def open_app(s, bare, base: str, name: str, hdr: dict) -> tuple[int | None, dict, str]:
    """GET /apps/{name}/ on the main host, then the 308's Location on the App's host: the base URL
    with that Host header, as app.server calls it (loopback only, no DNS), on `bare`, a session
    without the Portal's cookies (another origin). (the App host's status, its headers, what the
    row shows); status None when prod's 308 did not come, so no row passes on the main host."""
    st, hdrs, _ = s.call("GET", f"/apps/{urllib.parse.quote(name)}/", headers=hdr)
    loc, why = moved_to(base, name, st, hdrs)
    if not loc:
        return None, hdrs, why
    u = urllib.parse.urlsplit(loc)
    st, hdrs, _ = bare.call("GET", u.path + (f"?{u.query}" if u.query else ""), headers={**hdr, "Host": u.netloc})
    return st, hdrs, f"308 {loc}, there {st} {hdrs.get('content-type')}"


def no_shard_judge(status: int, headers: dict, raw: bytes, name: str) -> tuple[str, bool]:
    """(actual, ok) of the same call on an installation without a shard: the Portal's designed 503
    problem naming the App, which names no internal address (no shard host)."""
    ctype = headers.get("content-type", "")
    text = raw[:400].decode(errors="replace")
    actual = f"{status} {ctype or '-'} {text[:160]!r}"
    if status == 200 and ctype.startswith("text/html"):
        actual = "a page: is jc's App host layer on (a *.apps mount, REHEARSAL_APP_HOSTS=on)? " + actual
    ok = (status == 503 and ctype.startswith("application/problem+json")
          and f"the server of App '{name}'" in text and "svc" not in text and "wasm-host" not in text)
    return actual, ok


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


def judge_closed(status: int, frames, error: str) -> tuple[str, str, str]:
    """(result, actual, reason) of a daiya render, without a person, of a panel whose datasource
    passes the caller's token on (oauthPassThru) to a closed Endpoint: daiya then reads
    anonymously, and the gateway must refuse it. A row is a leak; the person's read is portal.panel."""
    rows = frame_rows(frames)
    actual = f"{status}, {len(frames or [])} frames, {rows} rows" + (f", error: {error}" if error else "")
    if rows:
        return FAIL, actual, "rows of a closed Endpoint read without a person token"
    if re.search(r"upstream status 40[13]\b", error or ""):
        return PASS, actual, "refused upstream: no person token for a closed Endpoint (portal.panel reads it as the person)"
    return FAIL, actual, "neither rows nor the gateway's refusal"


# The Portal's answer when it ignored X-Access-Token and read the cookie session instead.
NO_TOKEN = "does not carry"


def panel_answer(st: int, ans) -> tuple[list, str]:
    """(frames, error) of the Portal's panel answer: `error`, `accessDenied`, or a problem's detail."""
    ans = ans if isinstance(ans, dict) else {"error": str(ans)[:160]}
    err = ans.get("error") or ans.get("accessDenied") or (ans.get("detail") if st != 200 else "") or ""
    return ans.get("frames") or [], str(err)[:200]


def judge_reader(status: int, frames, error: str) -> tuple[str, str, str]:
    """(result, actual, reason) of a closed Endpoint's panel read through the Portal as a person
    the Endpoint's Policy grants, with their edge token: 200 and at least one row, nothing else."""
    rows = frame_rows(frames)
    actual = f"{status}, {len(frames or [])} frames, {rows} rows" + (f", error: {error}" if error else "")
    if status == 200 and not error and rows:
        return PASS, actual, ""
    if NO_TOKEN in (error or ""):
        return FAIL, actual, "the Portal ignored X-Access-Token: is JC_TRUST_EDGE_TOKEN on (deploy/rehearsal/apps.sh)?"
    return FAIL, actual, "no rows for a person the Policy grants" if status == 200 and not error else ""


def judge_refused(status: int, frames, error: str) -> tuple[str, str, str]:
    """(result, actual, reason) of the same read as a signed-in person the Portal lets open the
    dashboard and the Policy names nowhere: the Portal answers 200 with no frame and the gateway's
    refusal (403, or the 404 an outsider of the Endpoint's Organization gets, EP-14). A row is a leak."""
    rows = frame_rows(frames)
    actual = f"{status}, {len(frames or [])} frames, {rows} rows" + (f", error: {error}" if error else "")
    if rows or frames:
        return FAIL, actual, "LEAK: data of a closed Endpoint for a person no Policy grants"
    if status == 200 and re.search(r"upstream status 40[134]\b", error or ""):
        return PASS, actual, "refused upstream"
    return FAIL, actual, "neither frames nor the gateway's refusal"


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
    p.add_argument("--realm-token-file", help="a portal-reconciler token (else from --state)")
    p.add_argument("--person-token-file", help="unused by this suite (accepted for the common runner)")
    p.add_argument("--cache", help="where the export index is kept (default <out>/../.cache)")
    p.add_argument("--only", help="regex over project, dashboard uid or App name")
    p.add_argument("--no-shots", dest="shots", action="store_false")
    p.add_argument("--no-apps", dest="apps", action="store_false")
    p.add_argument("--timeout", type=float, default=60)
    p.add_argument("--no-wasm-host", dest="wasm_host", action="store_false",
                   help="the installation runs no wasm-host shard: app.server is a named skip and the Portal's "
                        "designed 503 is asserted instead (app.server.no-shard)")
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    export, out = Path(a.export), Path(a.out)
    state = Path(a.state) if a.state else None
    only = re.compile(a.only) if a.only else None
    res = Results("charts-apps", a.base_url)
    s = Session(a.base_url, a.timeout)

    def secret(name):
        # jc's state dir, else the files up.sh gives jc for the tokens jc keeps in memory only
        # (deploy/rehearsal/rehearsal.py read_secret)
        for f in ((state / name, state.parent / "secrets" / name) if state else ()):
            if f.exists():
                return f.read_text().strip()
        return None

    manifest = json.loads((export / "manifest.json").read_text())
    now = dt.datetime.now(dt.UTC)
    cutoff = manifest.get("temporalCutoff")
    since = max(now - dt.timedelta(hours=24),
                ex.parse_time(cutoff) if cutoff else now - dt.timedelta(hours=24))
    window = {"from": str(int(since.timestamp() * 1000)), "to": str(int(now.timestamp() * 1000))}
    meta = {"export": str(export), "exportedAt": manifest.get("exportedAt"),
            # a synthetic bundle (rehearsal.py synth) holds the deployment's config only, not the source's data
            "bundle": "synthetic" if manifest.get("synthetic") else "export", "window":
            f"{since.isoformat(timespec='seconds')} .. {now.isoformat(timespec='seconds')}"}

    # ---- the person and the Portal session
    signed, own_password = False, None
    realm = realm_of(a.state)
    try:
        tok = read_token(a.realm_token_file) or (admin_token(s, secret("portal-admin-client-secret"), realm)
                                                  if secret("portal-admin-client-secret") else None)
        if not tok:
            res.skip("session", "portal", "sign in", "a signed-in platform admin",
                     "no --realm-token-file and no portal-admin-client-secret in --state")
        else:
            own_password = make_person(s, tok, realm=realm)
            ok, detail = sign_in(s, own_password)
            res.check("session", "portal", "GET /api/v1/auth/login (code flow)", "signed in", detail, ok)
            signed = ok
    except RuntimeError as e:
        res.check("session", "portal", "sign in as smoke-charts-apps", "signed in", str(e)[:200], False)

    dirs = [export / "grafana" / "dashboards", *map(Path, a.dashboards)]
    dashboards, file_ds = inv.load_dashboards([d for d in dirs if d.exists()])
    endpoints = ex.load_endpoints(export / "gitea" / "files")
    slug_tenant = {e.slug: e.tenant for e in endpoints}
    closed = {e.slug for e in endpoints if e.closed}
    index = ex.load_index(export, endpoints, Path(a.cache) if a.cache else out.parent / ".cache", log=lambda m: print(m, file=sys.stderr))
    types = {t.get("entityType") for d in dashboards for pn in d.panels for t in pn.targets if t.get("entityType")}
    temporal = inv.temporal_index(export, types)
    daiya = Daiya(s, secret("dashboards-password"))
    browser = None
    shots_dir = out / "shots"
    app_realm = Realm(state)
    edge_tokens: dict = {}

    def edge_token(username: str):
        """(token, why not) of `username` at the edge client, minted once and held in memory only:
        prod's edge adds it to every request as X-Access-Token (JC_TRUST_EDGE_TOKEN)."""
        if username not in edge_tokens:
            try:
                pw = own_password if username == PERSON else None
                edge_tokens[username] = (app_token(a.base_url, app_realm, EDGE_CLIENT, username, a.timeout, pw), "")
            except RuntimeError as e:
                edge_tokens[username] = (None, str(e)[:200])
        return edge_tokens[username]
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
        no_prom = 0
        for pn in d.panels:
            counts["panels"] += 1
            ptgt = f"{tgt}#{pn.id} {pn.title[:50]}"
            if pn.query_free():
                res.skip("panel", ptgt, "-", "-", f"a {pn.type} panel runs no query")
                continue
            dsrc = dsx.get(pn.ds_uid) or {}
            is_prom = pn.ds_type == inv.PROMETHEUS or dsrc.get("type") == inv.PROMETHEUS or pn.ds_uid == inv.PROMETHEUS
            if is_prom and not any(x.get("type") == inv.PROMETHEUS for x in dsx.values()):
                no_prom += 1
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
                if (dsrc.get("jsonData") or {}).get("oauthPassThru") and slug in closed:
                    r, act, reason = judge_closed(st, frames, err)
                    daiya_exp = "refused upstream, no rows (closed Endpoint, no person token)"
                else:
                    (r, act, reason), daiya_exp = judge(st, frames, err, expect, why), exp_txt
                res.add("daiya.panel", ptgt, f"GET /dashboards/api/dashboards/{d.uid}/render?panel={pn.id}&from&to (org {org}, datasource {pn.ds_uid})",
                        daiya_exp, act, r, reason)
            elif daiya.auth:
                res.add("daiya.panel", ptgt, "render", exp_txt, "", FAIL, "the dashboard is not in daiya")
            # -- the same panel through the Portal
            if not signed:
                res.skip("portal.panel", ptgt, "GET /api/v1/projects/{p}/grafana/dashboards/{uid}/panels/{id}", exp_txt, "not signed in")
                continue
            q = urllib.parse.urlencode(window)
            path = f"/api/v1/projects/{d.project}/grafana/dashboards/{urllib.parse.quote(d.uid)}/panels/{pn.id}?{q}"
            if (dsrc.get("jsonData") or {}).get("oauthPassThru") and slug in closed:
                # A closed Endpoint's panel is read with the person's own token (the cookie session
                # carries none by design): the edge's X-Access-Token, as prod's edge sends it.
                for check_id, who, judged, want in (
                        ("portal.panel", READER, judge_reader, "200 and rows (the Policy grants the reader's group)"),
                        ("portal.panel.refused", PERSON, judge_refused,
                         "200, no frame, the gateway's refusal (no Policy names this person)")):
                    token, why_not = edge_token(who)
                    call = f"GET {path.split('?')[0]} as {who} (X-Access-Token of {EDGE_CLIENT})"
                    if not token:
                        res.add(check_id, ptgt, call, want, "", FAIL, f"no edge token: {why_not}")
                        continue
                    st, ans = s.json("GET", path, headers={"X-Access-Token": token})
                    res.add(check_id, ptgt, call, want, *judged(st, *panel_answer(st, ans)))
                continue
            st, ans = s.json("GET", path)
            frames, err = panel_answer(st, ans)
            r, act, reason = judge(st, frames, err, expect, why)
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
            queried = sum(1 for pn in d.panels if not pn.query_free())
            for row in browser.dashboard(d.project, d.uid, shots_dir):
                bad = [x for x in row.get("panelAnswers", []) if x >= 400]
                drawn = (row.get("status") == 200 and not row.get("error")
                         and d.title.split(" ")[0].lower() in (row.get("h1") or "").lower())
                actual = (f"{row.get('status')}, h1 {row.get('h1')!r}, alerts {row.get('alerts')}, panel reads "
                          f"{len(row.get('panelAnswers', []))} ({len(bad)} failed){', ' + row['error'] if row.get('error') else ''}; "
                          f"shot {Path(row['shot']).relative_to(out) if row.get('shot') else '-'}")
                call = f"browser {row['url']} at {row['width']} px"
                want = f"the page draws '{d.title}', no alert, no failed panel read"
                if drawn and queried and no_prom == queried:
                    # every panel waits on a Prometheus the rehearsal does not run: the page is drawn, its data cannot be
                    res.add(f"shot.{row['width']}", tgt, call, want, actual, SKIP,
                            f"page drawn; all {queried} panels query Prometheus, which the rehearsal does not run")
                    continue
                res.check(f"shot.{row['width']}", tgt, call, want, actual, drawn and not row.get("alerts") and not bad)

    # ---- Apps
    if a.apps:
        apps = inv.load_apps(export / "gitea" / "files")
        repos = [r for r in json.loads((export / "gitea" / "repos.json").read_text()) if r["owner"] == APPS_OWNER]
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
                actual = f"{st}"
                if st == 404 and owner != APPS_OWNER:
                    # say where the repository went when the import put it under the apps owner
                    ost, _ = s.json("GET", f"/git/api/v1/projects/{APPS_OWNER}/repositories/{repo}", headers=sauth)
                    actual += f"; {APPS_OWNER}/{repo} answers {ost}"
                res.check("app.source", tgt, call, "200 (the App's spec.source.git is in shako)", actual, st == 200)
            if app.lifecycle != "published":
                res.skip("app.served", tgt, f"GET /apps/{app.name}/", "-", f"lifecycle {app.lifecycle!r}: not served by design")
                continue
            call = f"GET /apps/{app.name}/"
            if app.git and not app.built:
                for check_id in ("app.served", "app.page"):
                    res.skip(check_id, tgt, call, "-", NEVER_BUILT)
                continue
            if app.kind in KEPT:
                for check_id in ("app.served", "app.page"):
                    res.skip(check_id, tgt, call, "-", KEPT[app.kind])
                continue
            # Who opens it: anyone for a public App; else a person on a token of the App's own
            # client, which the edge holds on prod: rehearsal-app-roles (the roles Apps' groups)
            # gets in, rehearsal-tester (no group) is refused: 403 for a roles App, 404 otherwise.
            token, moved = None, shots_app_host(a.base_url, app.name) + "/"
            if app.visibility != "public":
                try:
                    refused = app_token(a.base_url, app_realm, f"app-{app.name}", TESTER, a.timeout)
                    token = app_token(a.base_url, app_realm, f"app-{app.name}", HOLDER, a.timeout)
                except RuntimeError as e:
                    res.check("app.served", tgt, call, "a person's token of the App's client", str(e)[:200], False)
                    continue
                want = 403 if app.visibility == "roles" else 404
                st, _, actual = open_app(s, Session(a.base_url, a.timeout), a.base_url, app.name,
                                         {"Authorization": f"Bearer {refused}"})
                res.check("app.refused", tgt, f"{call} as {TESTER} (no group), then on the App's host",
                          f"308 to {moved}, there {want}", actual, st == want)
            hdr = {"Authorization": f"Bearer {token}"} if token else {}
            st, hdrs, actual = open_app(s, Session(a.base_url, a.timeout), a.base_url, app.name, hdr)
            served = st == 200 and "html" in hdrs.get("content-type", "")
            who = f" as {HOLDER}" if token else " anonymously"
            res.check("app.served", tgt, call + who + ", then on the App's host", f"308 to {moved}, there 200 text/html",
                      actual, served)
            if not served:
                res.skip("app.page", tgt, f"browser /apps/{app.name}/", "main page reads data", "not served (app.served failed)")
                continue
            if app.kind == "wasm":
                # On the App's own host, without the Portal's cookies (another origin).
                bare = Session(a.base_url, a.timeout)
                host = {"Host": urllib.parse.urlparse(shots_app_host(a.base_url, app.name)).netloc}
                call = f"GET {host['Host']}/api/me"
                if token:
                    st, _, _ = bare.call("GET", "/api/me", headers=host)
                    res.check("app.server.signed-out", tgt, call + " anonymously", "401, nothing handed to the shard",
                              f"{st}", st == 401)
                    token = app_token(a.base_url, app_realm, f"app-{app.name}", HOLDER, a.timeout)
                    host["Authorization"] = f"Bearer {token}"
                st, hdrs, raw = bare.call("GET", "/api/me", headers=host)
                want = f"200 application/json from the App's shard: {'the caller' if token else 'null'}"
                if a.wasm_host:
                    actual, ok = server_judge(st, hdrs, raw, bool(token))
                    res.check("app.server", tgt, call + who, want, actual, ok)
                else:
                    actual, ok = no_shard_judge(st, hdrs, raw, app.name)
                    res.check("app.server.no-shard", tgt, call + who,
                              "503 application/problem+json naming the App, no internal address", actual, ok)
                    res.skip("app.server", tgt, call + who, want, NO_SHARD)
                res.skip("app.page", tgt, f"browser /apps/{app.name}/", "main page reads data", WASM_PAGE)
                continue
            if not a.shots:
                res.skip("app.page", tgt, f"browser /apps/{app.name}/", "main page reads data", "--no-shots")
                continue
            if browser is None:
                from shots import Browser
                browser = Browser(a.base_url, s.jar)
            if token:  # a fresh one: the App's tokens live 60 s
                token = app_token(a.base_url, app_realm, f"app-{app.name}", HOLDER, a.timeout)
            login = (HOLDER, app_realm.password(HOLDER)) if token and app_realm.password(HOLDER) else None
            for row in browser.app(app.name, shots_dir, token, login):
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
