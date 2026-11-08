#!/usr/bin/env python3
"""UI and feature parity between the old component on dev and its jc module in a rehearsal.

  parity.py capture --side old --base https://dev.example --pairs ckan,grafana --out <dir> [--person-file f]
  parity.py capture --side new --base http://localhost:42240 --state <rehearsal state> --out <dir>
  parity.py compose --out <dir>        # side-by-side PNGs (old | new) and features.md per pair

The old side is read only: every request to it other than GET/HEAD/OPTIONS is aborted, except the
sign-in form post to its Keycloak. Each page is shot at 1440 and 400 px and fingerprinted (headings,
features from pages.py); compose turns both fingerprints into one feature table per pair.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import urljoin, urlparse

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pages as P  # noqa: E402

WIDTHS = ((1440, 900), (400, 860))
MAX_HEIGHT = 4000  # px of a full-page shot kept; ponytail: longer pages are cut, scroll shots if needed


def fingerprint(page, features: list[dict], side: str) -> dict:
    heads = page.eval_on_selector_all(
        "h1,h2,h3", "els=>els.filter(e=>e.offsetParent!==null).map(e=>e.tagName+' '+e.innerText.trim().slice(0,80))")
    rows = []
    for f in features:
        sel = f.get(side) or f.get("sel")
        if not sel:
            rows.append({"feature": f["name"], "present": None, "element": "-"})
            continue
        try:
            n = sum(fr.locator(sel).count() for fr in page.frames if fr.url.startswith('http'))  # embedded grids and maps live in iframes
        except Exception as e:  # a bad selector reads as missing, with the reason
            rows.append({"feature": f["name"], "present": False, "element": sel, "error": str(e)[:120]})
            continue
        row = {"feature": f["name"], "present": n > 0, "count": n, "element": sel}
        if f.get("texts") and n:
            row["texts"] = [t.strip()[:80] for t in page.locator(sel).all_inner_texts()[:60]]
        rows.append(row)
    return {"headings": heads, "features": rows}


def guard_old(route, request, keycloak_host: str):
    """Aborts every write to dev but the sign-in form post."""
    if request.method in ("GET", "HEAD", "OPTIONS"):
        return route.continue_()
    if urlparse(request.url).hostname == keycloak_host and "/login-actions/authenticate" in request.url:
        return route.continue_()
    if urlparse(request.url).hostname in ("localhost", "127.0.0.1"):  # the local Gitea stand-in, never dev
        return route.continue_()
    return route.abort()


def sign_in(page, side: str, base: str, person: tuple[str, str]):
    user, password = person
    login = urljoin(base, "/api/v1/auth/login")
    page.goto(login)
    page.fill("#username" if side == "old" else "input[name=username]", user)
    page.fill("#password" if side == "old" else "input[name=password]", password)
    page.locator("#password" if side == "old" else "input[name=password]").press("Enter")
    page.wait_for_url(re.compile(r".*/(projects|catalogue|home)(/.*)?$|.*/$"), timeout=45000)


def capture(a):
    from playwright.sync_api import sync_playwright

    out = Path(a.out)
    side = a.side
    pairs = a.pairs.split(",") if a.pairs else list(P.PAIRS)
    person = None
    if a.person_file:
        user, password = Path(a.person_file).read_text().split()[:2]
        person = (user, password)
    blocked: list[str] = []
    with sync_playwright() as pw:
        browser = pw.chromium.launch(args=["--disable-dev-shm-usage", "--disable-gpu"])
        for width, height in WIDTHS:
            ctx = browser.new_context(viewport={"width": width, "height": height}, locale="sk-SK")
            if side == "old":
                kc = urlparse(P.OLD_KEYCLOAK).hostname
                ctx.route("**/*", lambda route, req: (blocked.append(f"{req.method} {req.url[:120]}")
                                                       if req.method not in ("GET", "HEAD", "OPTIONS") else None)
                          or guard_old(route, req, kc))
            page = ctx.new_page()
            if person:
                sign_in(page, side, P.portal_base(side, a.base), person)
            for pair in pairs:
                spec = P.PAIRS[pair]
                try:
                    ctxvars = P.resolve(pair, side, a.base, page)
                except Exception as e:  # noqa: BLE001 - unresolved names turn the pages into skips with the reason
                    print(f"resolve {pair}: {str(e)[:200]}")
                    ctxvars = {"project": P.PROJECT, "dataset": P.DATASET, "org": P.PROJECT}
                varfile = out / pair / f"vars-{side}.json"
                varfile.parent.mkdir(parents=True, exist_ok=True)
                varfile.write_text(json.dumps(ctxvars, indent=1))
                if side == "new" and (out / pair / "vars-old.json").exists():
                    # Same objects on both sides: the old side's names, the new side's own where it found one.
                    ctxvars = {**json.loads((out / pair / "vars-old.json").read_text()), **ctxvars}
                for pg in spec["pages"]:
                    path = pg[side]
                    if path is None:
                        _write(out, pair, side, pg["id"], width, {"status": None, "skip": pg.get("why", "no such page")})
                        continue
                    try:
                        path = path.format(**ctxvars)
                    except KeyError as e:
                        _write(out, pair, side, pg["id"], width, {"status": None, "skip": f"no {e} on this side"})
                        continue
                    url = urljoin(P.base_of(pair, side, a.base), P.old_path(pair, path) if side == "old" else path)
                    failed: list[str] = []
                    hook = lambda r: failed.append(f"{r.status} {r.request.method} {r.url[:140]}") if r.status >= 400 else None
                    tab = page
                    if pg.get("anon"):
                        anon = browser.new_context(viewport={"width": width, "height": height}, locale="sk-SK")
                        if side == "old":
                            anon.route("**/*", lambda route, req: guard_old(route, req, ""))
                        tab = anon.new_page()
                    tab.on("response", hook)
                    try:
                        resp = tab.goto(url, wait_until="networkidle", timeout=60000)
                        status = resp.status if resp else None
                    except Exception as e:
                        status = f"error: {str(e)[:100]}"
                    for act in pg.get("then", []):
                        try:
                            P.act(tab, act, side)
                        except Exception as e:
                            failed.append(f"action {act}: {str(e)[:100]}")
                    tab.wait_for_timeout(pg.get("settle", 2500))
                    shot = out / pair / "raw" / side / f"{pg['id']}-{width}.png"
                    shot.parent.mkdir(parents=True, exist_ok=True)
                    full = min(int(tab.evaluate("document.documentElement.scrollHeight") or height), MAX_HEIGHT)
                    try:
                        tab.set_viewport_size({"width": width, "height": max(full, height)})
                        tab.wait_for_timeout(500)
                        tab.screenshot(path=str(shot), timeout=60000)
                    except Exception as e:
                        failed.append(f"screenshot: {str(e)[:100]}")
                    tab.set_viewport_size({"width": width, "height": height})
                    fp = fingerprint(tab, pg.get("features", []), side)
                    tab.remove_listener("response", hook)
                    _write(out, pair, side, pg["id"], width,
                           {"url": tab.url, "status": status, "failed": sorted(set(failed))[:12], **fp})
                    if tab is not page:
                        tab.context.close()
                    print(f"{side} {width:4} {pair:8} {pg['id']:22} {status}", flush=True)
            ctx.close()
        browser.close()
    if side == "old":
        (out / "old-blocked-writes.txt").write_text("\n".join(sorted(set(blocked))) + "\n")


def _write(out: Path, pair, side, pid, width, row):
    f = out / pair / "raw" / side / f"{pid}-{width}.json"
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text(json.dumps(row, indent=1, ensure_ascii=False))


def _load(out: Path, pair, side, pid, width):
    f = out / pair / "raw" / side / f"{pid}-{width}.json"
    return json.loads(f.read_text()) if f.exists() else None


def side_by_side(old: Path, new: Path, dest: Path, label: str):
    """old | new, each cut at MAX_HEIGHT, with a label bar over each half."""
    args = ["magick"]
    for img, name in ((old, "OLD (dev)"), (new, "NEW (jc rehearsal)")):
        if img.exists():
            args += ["(", str(img), "-crop", f"x{MAX_HEIGHT}+0+0", "+repage"]
        else:
            args += ["(", "-size", "400x300", "xc:#eeeeee", "-gravity", "center", "-pointsize", "20",
                     "-annotate", "0", "no page"]
        args += ["-background", "#222", "-fill", "white", "-pointsize", "22", "-gravity", "north",
                 "-splice", "0x36", "-annotate", "+0+6", f"{name}  {label}", ")"]
    args += ["-background", "white", "-gravity", "north", "+append", str(dest)]
    subprocess.run(args, check=True)


def verdict_row(o, n, f_old, f_new):
    if o is None and n is None:
        return "n/a"
    if o and not n:
        return "missing"
    if n and not o:
        return "extra"
    return "same"


def compose(a):
    out = Path(a.out)
    pairs = a.pairs.split(",") if a.pairs else list(P.PAIRS)
    for pair in pairs:
        spec = P.PAIRS[pair]
        lines = [f"# {spec['title']}: feature table", "",
                 "Generated by bins/jc/smoke/parity/parity.py compose. `present` means the element is on the page at 1440 px.", "",
                 "| page | feature | old | new | result | element (old / new) |", "|---|---|---|---|---|---|"]
        heads = ["", "## Headings on old missing on new (1440)", ""]
        for pg in spec["pages"]:
            for width, _ in WIDTHS:
                o = out / pair / "raw" / "old" / f"{pg['id']}-{width}.png"
                n = out / pair / "raw" / "new" / f"{pg['id']}-{width}.png"
                if o.exists() or n.exists():
                    side_by_side(o, n, out / pair / f"{pg['id']}-{width}.png", f"{pg['id']} @{width}")
            jo, jn = _load(out, pair, "old", pg["id"], 1440), _load(out, pair, "new", pg["id"], 1440)
            fo = {r["feature"]: r for r in (jo or {}).get("features", [])}
            fn = {r["feature"]: r for r in (jn or {}).get("features", [])}
            for feat in pg.get("features", []):
                ro, rn = fo.get(feat["name"], {}), fn.get(feat["name"], {})
                po, pn = ro.get("present"), rn.get("present")
                res = verdict_row(po, pn, ro, rn)
                if jn and jn.get("skip"):
                    res = "missing"
                lines.append(f"| {pg['id']} | {feat['name']} | {_yn(po, jo)} | {_yn(pn, jn)} | {res} | "
                             f"`{ro.get('element', '-')}` / `{rn.get('element', '-')}` |")
            if jo and jn:
                norm = lambda h: re.sub(r"\s+\d[\d\s.,]*$", "", h.lower())
                new_h = {norm(h) for h in jn.get("headings", [])}
                gone = [h for h in jo.get("headings", []) if norm(h) not in new_h]
                if gone:
                    heads.append(f"- {pg['id']}: " + "; ".join(gone[:15]))
            for side, j in (("old", jo), ("new", jn)):
                if j and (j.get("failed") or j.get("skip") or (isinstance(j.get("status"), int) and j["status"] >= 400)):
                    heads.append(f"- {side} {pg['id']}: status {j.get('status')} {j.get('skip', '')} failed {j.get('failed', [])[:5]}")
        (out / pair / "features.md").write_text("\n".join(lines + heads) + "\n")
        print(f"{pair}: {out / pair / 'features.md'}")


def report(out: Path, pairs) -> list[dict]:
    """One check per page status and per feature the old page has: the new page must have it too."""
    rows = []
    for pair in pairs:
        for pg in P.PAIRS[pair]["pages"]:
            for width, _ in WIDTHS:
                jo, jn = _load(out, pair, "old", pg["id"], width), _load(out, pair, "new", pg["id"], width)
                target = f"{pair}/{pg['id']}@{width}"
                call = f"GET {pg.get('new')}"
                if jo is None or jo.get("skip") or not isinstance(jo.get("status"), int) or jo["status"] >= 400:
                    why = "old page not shot" if jo is None else (jo.get("skip") or f"old page answered {jo.get('status')}")
                    rows.append({"id": f"{target}/page", "target": target, "call": call, "expected": "the old page",
                                 "actual": "-", "result": "skip", "reason": why})
                    continue
                if jn is None or jn.get("skip"):
                    rows.append({"id": f"{target}/page", "target": target, "call": call, "expected": "a page like the old one",
                                 "actual": (jn or {}).get("skip", "not shot"), "result": "fail"})
                    continue
                ok = isinstance(jn.get("status"), int) and jn["status"] < 400
                rows.append({"id": f"{target}/page", "target": target, "call": call, "expected": f"status {jo['status']}",
                             "actual": f"status {jn.get('status')}", "result": "pass" if ok else "fail"})
                if width != 1440:
                    continue
                fo = {r["feature"]: r for r in jo.get("features", [])}
                fn = {r["feature"]: r for r in jn.get("features", [])}
                for name, ro in fo.items():
                    rn = fn.get(name, {})
                    if not ro.get("present"):
                        rows.append({"id": f"{target}/{name}", "target": target, "call": call, "expected": name, "actual": "-",
                                     "result": "skip", "reason": "the old page has no such element either"})
                        continue
                    rows.append({"id": f"{target}/{name}", "target": target, "call": call, "expected": f"{name} ({ro.get('element')})",
                                 "actual": f"{rn.get('count', 0)} x {rn.get('element')}", "result": "pass" if rn.get("present") else "fail"})
    return rows


def write_report(out: Path, rows: list[dict]):
    (out / "results.json").write_text(json.dumps(rows, indent=1, ensure_ascii=False))
    n = {k: sum(r["result"] == k for r in rows) for k in ("pass", "fail", "skip")}
    lines = ["# Smoke suite parity", "", f"{n['pass']} pass, {n['fail']} fail, {n['skip']} skip.", "",
             "| id | call | expected | actual | result |", "|---|---|---|---|---|"]
    for r in rows:
        if r["result"] != "pass":
            lines.append(f"| {r['id']} | `{r['call']}` | {r['expected']} | {r['actual']} | {r['result']} {r.get('reason', '')} |")
    (out / "REPORT.md").write_text("\n".join(lines) + "\n")
    return n


def _yn(p, j):
    if j is None:
        return "not shot"
    if j.get("skip"):
        return "skip: " + j["skip"]
    return {True: "yes", False: "no", None: "-"}[p]


def main(argv=None):
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("capture")
    c.add_argument("--side", choices=("old", "new"), required=True)
    c.add_argument("--base", required=True, help="old: dev's primary host; new: the rehearsal's jc listener")
    c.add_argument("--pairs")
    c.add_argument("--person-file", help="file holding '<user> <password>'; never printed")
    c.add_argument("--out", required=True)
    m = sub.add_parser("compose")
    m.add_argument("--pairs")
    m.add_argument("--out", required=True)
    a = ap.parse_args(argv)
    {"capture": capture, "compose": compose}[a.cmd](a)


if __name__ == "__main__":
    main()
