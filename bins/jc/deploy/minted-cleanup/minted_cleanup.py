#!/usr/bin/env python3
"""minted_cleanup.py: deletes the copies jc-prod's pipelines minted under the wrong org domain
(T-6676 R) through each space's Endpoint, as the temporary cleanup identity. Stdlib only.

  minted_cleanup.py run --list L.tsv --endpoints E.tsv --state DIR [--only TENANT]...
                        [--apply --gateway URL --mem-cmd CMD [--max-batches N]
                         (--token-file F | --token-url URL --client-secret-file F [--client-id ID])]
  minted_cleanup.py policies --list L.tsv --endpoints E.tsv --state DIR --until ISO [--owner USER-ID]
  minted_cleanup.py verify --recount R.tsv --history H.tsv --state DIR

L.tsv (sql/list.sql, read-only): tenant, id, types (space-separated), expires_at (ISO or
empty), refs (the minted ids the entity's document names, space-separated). E.tsv: tenant, the
Endpoint's slug, the space that Endpoint names (spec.contextSpaceRef, read from the Portal), the
project. `policies` writes per space the temporary Policy (README.md) for the types listed and,
with --owner, per project the temporary ServiceAccount: role `public` (the anonymous reader's)
in each of its spaces until --until, since jc-core refuses an account without a role.

`run` checks every row first: the id lies under urn:ngsi-ld:<Type>:portal.smartcity.joinedcontext.com:,
<Type> is one of the row's types, the row's tenant is the space of its group's Endpoint. A group
with one failing row stops the whole run before any delete. Copies are deleted in layers: a copy
goes only once no remaining copy names it (the gateway plans its relationship rules per id and
does not count ids of the same batch), in batches of at most 500 ids. Without --apply nothing is
sent. With it, each batch goes to POST <gateway>/api/endpoint/<slug>/ngsi-ld/v1/entityOperations/delete;
ResourceNotFound counts as done only for an expired copy or one an interrupted batch sent; any
other member error, any other answer and memory at or above 95 % (the last number --mem-cmd
prints) stop the run. DIR keeps done.txt and the batch in flight, so a rerun resumes; --max-batches N
pauses the run after N batches (a canary: exit 0, state kept for the resume). The token is read
from --token-file per batch, or, with --client-secret-file, fetched by client credentials of
--client-id (default <project>-cleanup-t6676) from --token-url and fetched again once half its
expires_in has passed (the secret read from its 0600 file per fetch); neither comes from argv or
the environment, and neither is logged or kept in DIR.

`verify` reads the recount (tenant, id, expires_at) and the history count (tenant, id, then the
rows of temporal_entities, attr_instances, attr_versions and attr_rows) under the prefix: non-expired copies left fail it; when history remains, it
lists expired copies for the broker's expiry sweep and deleted ids whose history stayed.
"""

from __future__ import annotations

import argparse
import datetime as dt
import ipaddress
import json
import os
import re
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict
from pathlib import Path

DOMAIN = "portal.smartcity.joinedcontext.com"
ID = re.compile(r"urn:ngsi-ld:([A-Za-z]+):" + re.escape(DOMAIN) + r":\S+")
BATCH = 500
MEM_STOP = 95.0
ACCOUNT = "cleanup-t6676"
LABEL = "T-6676 R"
DEFAULTS = (DOMAIN, ACCOUNT, LABEL)  # the T-6676 R run; main() starts from these every call
REASON = f"cleanup: copies minted under the wrong org domain ({LABEL})"
TIMEOUT = 300
SLUG = re.compile(r"[a-z0-9-]{1,63}")


class Stop(Exception):
    """Ends the run; the message is the log's last line and the exit status is 1."""


def log(state: Path | None, line: str) -> None:
    print(line, flush=True)
    if state:
        with open(state / "log.txt", "a", encoding="utf-8") as f:
            f.write(f"{dt.datetime.now(dt.UTC):%Y-%m-%dT%H:%M:%SZ} {line}\n")


def tsv(path: str, width: int) -> list[list[str]]:
    rows = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            line = line.rstrip("\n")
            if not line:
                continue
            cols = line.split("\t")
            if len(cols) < width:
                cols += [""] * (width - len(cols))
            if len(cols) != width:
                raise Stop(f"{path}:{n}: {len(cols)} columns, {width} expected")
            rows.append(cols)
    return rows


def short(type_iri: str) -> str:
    return re.split(r"[/#]", type_iri)[-1]


def when(text: str) -> dt.datetime | None:
    if not text:
        return None
    try:
        t = dt.datetime.fromisoformat(text)
    except ValueError:
        t = None
    if t is None or t.tzinfo is None:
        raise Stop(f"{text!r} is no ISO time with a zone")
    return t


def load(list_path: str, endpoints_path: str, only: list[str]) -> dict[str, dict]:
    """The groups by tenant, each checked whole: a failing row raises before anything is sent."""
    endpoints = {}
    for tenant, slug, space, project in tsv(endpoints_path, 4):
        if not SLUG.fullmatch(slug):
            raise Stop(f"endpoint of {tenant}: {slug!r} is no Endpoint slug")
        endpoints[tenant] = (slug, space, project)
    groups: dict[str, dict] = {}
    for tenant, eid, types, expires, refs in tsv(list_path, 5):
        if only and tenant not in only:
            continue
        g = groups.setdefault(tenant, {"rows": {}, "errors": []})
        m = ID.fullmatch(eid)
        if not m:
            g["errors"].append(f"{eid}: not under urn:ngsi-ld:<Type>:{DOMAIN}:")
        elif m.group(1) not in {short(t) for t in types.split()}:
            g["errors"].append(f"{eid}: its type {m.group(1)} is not the row's ({types})")
        if eid in g["rows"]:
            g["errors"].append(f"{eid}: listed twice")
        g["rows"][eid] = {"expires": when(expires), "refs": set(refs.split()) - {eid},
                          "type": m.group(1) if m else ""}
    for tenant, g in groups.items():
        if tenant not in endpoints:
            g["errors"].append(f"{tenant}: no Endpoint in {endpoints_path}")
            continue
        slug, space, g["project"] = endpoints[tenant]
        if space != tenant:
            g["errors"].append(f"{tenant}: its Endpoint {slug} serves the space {space}")
        g["slug"] = slug
    bad = {t: g["errors"] for t, g in groups.items() if g["errors"]}
    if bad:
        for t, errors in sorted(bad.items()):
            for e in errors[:20]:
                log(None, f"refused {t}: {e}")
            log(None, f"refused {t}: {len(errors)} row(s) fail")
        raise Stop(f"stopped before any delete: {len(bad)} group(s) fail their checks")
    return groups


def layers(rows: dict[str, dict], done: set[str]) -> list[list[str]]:
    """Kahn over 'is named by': a copy no remaining copy names goes first. A cycle stops."""
    left = {i for i in rows if i not in done}
    named_by: dict[str, set[str]] = defaultdict(set)
    for i in left:
        for r in rows[i]["refs"] & left:
            named_by[r].add(i)
    out = []
    while left:
        free = sorted(i for i in left if not named_by[i] & left)
        if not free:
            raise Stop(f"{len(left)} copies name each other in a cycle; first {min(left)}")
        out.append(free)
        left -= set(free)
    return out


def batches(layer: list[str]) -> list[list[str]]:
    return [layer[i:i + BATCH] for i in range(0, len(layer), BATCH)]


def read_ids(path: Path) -> set[str]:
    return set(path.read_text(encoding="utf-8").split()) if path.exists() else set()


def append_ids(path: Path, ids: list[str]) -> None:
    with open(path, "a", encoding="utf-8") as f:
        f.write("".join(i + "\n" for i in ids))
        f.flush()
        os.fsync(f.fileno())


def write_ids(path: Path, ids: set[str]) -> None:
    path.write_text("".join(i + "\n" for i in sorted(ids)), encoding="utf-8")


def secret_of(path: str) -> str:
    st = os.stat(path)
    if st.st_mode & (stat.S_IRWXG | stat.S_IRWXO):
        raise Stop(f"{path} is readable by others; chmod 600 it")
    value = Path(path).read_text(encoding="utf-8").strip()
    if not value or any(c.isspace() for c in value):
        raise Stop(f"{path} holds no single secret")
    return value


def fetch_token(token_url: str, client_id: str, secret_file: str) -> tuple[str, float]:
    """A client-credentials access token of client_id and its lifetime in seconds (300 when the
    answer names none); the secret goes in the form body only."""
    form = urllib.parse.urlencode({"grant_type": "client_credentials", "client_id": client_id,
                                   "client_secret": secret_of(secret_file)}).encode()
    req = urllib.request.Request(token_url, data=form, method="POST",
                                 headers={"Content-Type": "application/x-www-form-urlencoded",
                                          "Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            body = json.loads(r.read())
    except urllib.error.HTTPError as e:
        raise Stop(f"token request for {client_id}: answer {e.code}; nothing more is sent") from None
    except (OSError, ValueError) as e:
        raise Stop(f"token request for {client_id}: {type(e).__name__}; nothing more is sent") from None
    token = body.get("access_token") if isinstance(body, dict) else None
    if not isinstance(token, str) or not token or any(c.isspace() for c in token):
        raise Stop(f"token request for {client_id}: no access_token in the answer")
    life = body.get("expires_in")
    return token, float(life) if isinstance(life, (int, float)) and life > 0 else 300.0


def check_gateway(url: str) -> str:
    u = urllib.parse.urlparse(url)
    loop = False
    try:
        loop = u.hostname == "localhost" or ipaddress.ip_address(u.hostname or "").is_loopback
    except ValueError:
        pass
    if u.scheme != "https" and not (u.scheme == "http" and loop):
        raise Stop(f"{url}: https only (plain http to loopback for a stub)")
    return url.rstrip("/")


def memory(cmd: str) -> float:
    p = subprocess.run(["sh", "-c", cmd], check=False, capture_output=True, text=True, timeout=120)
    nums = re.findall(r"\d+(?:\.\d+)?", p.stdout)
    if p.returncode != 0 or not nums:
        raise Stop(f"memory check failed (exit {p.returncode}, no number): nothing more is sent")
    return float(nums[-1])


def post(gateway: str, slug: str, token: str, ids: list[str]) -> tuple[int, dict]:
    req = urllib.request.Request(
        f"{gateway}/api/endpoint/{slug}/ngsi-ld/v1/entityOperations/delete",
        data=json.dumps(ids).encode(), method="POST",
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json",
                 "Accept": "application/json", "JC-Reason": REASON})
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            status, raw = r.status, r.read()
    except urllib.error.HTTPError as e:
        status, raw = e.code, e.read()
    try:
        body = json.loads(raw) if raw else {}
    except ValueError:
        body = {}
    return status, body if isinstance(body, dict) else {"success": body}


def not_found(err: dict) -> bool:
    t = str((err.get("error") or {}).get("type", ""))
    return t == "ResourceNotFound" or t.endswith("/ResourceNotFound")


def judge(ids: list[str], status: int, body: dict, rows: dict, inflight: set[str],
          now: dt.datetime) -> tuple[list[str], int, str | None]:
    """(ids done, of them expired or resent, the first stop) for one batch's answer."""
    if status in (200, 204):
        return ids, 0, None
    if status == 401:
        return [], 0, "401: the token expired or was refused; refresh the token file (or check the client secret) and rerun"
    if status != 207:
        problem = f"{body.get('type', '')} {body.get('title', '')}".strip()
        return [], 0, f"answer {status} {problem}: nothing of this batch counted"
    sent = set(ids)
    done = [i for i in body.get("success", []) if i in sent]
    gone, stop = 0, None
    for err in body.get("errors", []):
        eid = err.get("entityId")
        exp = rows.get(eid, {}).get("expires")
        if eid in sent and not_found(err) and ((exp and exp <= now) or eid in inflight):
            done.append(eid)
            gone += 1
        elif stop is None:
            e = err.get("error") or {}
            why = "not found though not expired" if not_found(err) else f"{e.get('type', '')} {e.get('title', '')}"
            stop = f"member error on {eid}: {why.strip()}"
    return done, gone, stop


def run(a) -> int:
    groups = load(a.list, a.endpoints, a.only)
    state = Path(a.state)
    state.mkdir(mode=0o700, parents=True, exist_ok=True)
    done = read_ids(state / "done.txt")
    # Ids a batch sent without an answer (a timeout, a 5xx): a later ResourceNotFound is theirs.
    inflight = read_ids(state / "inflight.txt") - done
    plans = {}
    for tenant, g in sorted(groups.items()):
        plans[tenant] = layers(g["rows"], done)
        left = sum(map(len, plans[tenant]))
        log(state, f"{tenant}: {len(g['rows'])} listed, {len(g['rows']) - left} done before, "
                   f"{left} to delete in {len(plans[tenant])} layer(s), "
                   f"{sum(len(batches(l)) for l in plans[tenant])} batch(es) via {g['slug']}")
    if not a.apply:
        log(state, "dry run: nothing sent")
        return 0
    gateway = check_gateway(a.gateway)
    token_url = check_gateway(a.token_url) if a.token_url else None
    projects = sorted({g["project"] for g in groups.values()})
    if len(projects) > 1:  # one token is one project's ServiceAccount
        raise Stop(f"--apply serves one project's account; these tenants span {projects}: add --only")
    totals = {"deleted": 0, "gone": 0}
    sent = 0
    held: dict[str, tuple[str, float]] = {}  # client id -> (token, monotonic time to refetch at)
    for tenant, plan in sorted(plans.items()):
        g = groups[tenant]
        for n, layer in enumerate(plan):
            for b, ids in enumerate(batches(layer), 1):
                if a.max_batches is not None and sent >= a.max_batches:
                    log(state, f"paused after {sent} batch(es) (--max-batches {a.max_batches}): "
                               f"{totals['deleted']} deleted, {totals['gone']} already gone; rerun to resume")
                    return 0
                pct = memory(a.mem_cmd)
                if pct >= MEM_STOP:
                    raise Stop(f"memory {pct:g} % >= {MEM_STOP:g} %: stopped; rerun to resume")
                if token_url:
                    client = a.client_id or f"{g['project']}-{ACCOUNT}"
                    token, renew = held.get(client, ("", 0.0))
                    if time.monotonic() >= renew:
                        token, life = fetch_token(token_url, client, a.client_secret_file)
                        held[client] = (token, time.monotonic() + life / 2)
                else:
                    token = secret_of(a.token_file)
                sent += 1
                write_ids(state / "inflight.txt", inflight | set(ids))
                try:
                    status, body = post(gateway, g["slug"], token, ids)
                except (OSError, TimeoutError) as e:
                    raise Stop(f"{tenant} layer {n} batch {b}: {type(e).__name__}; rerun to resume") from None
                ok, gone, stop = judge(ids, status, body, g["rows"], inflight, dt.datetime.now(dt.UTC))
                append_ids(state / "done.txt", ok)
                totals["deleted"] += len(ok) - gone
                totals["gone"] += gone
                log(state, f"{tenant} layer {n} batch {b}: {len(ids)} sent, {len(ok) - gone} deleted, "
                           f"{gone} already gone, memory {pct:g} %")
                # A 5xx may have deleted part of the batch; any other answer is final.
                inflight = (inflight | set(ids) if status >= 500 else inflight - set(ids)) - set(ok)
                write_ids(state / "inflight.txt", inflight)
                if stop:
                    raise Stop(f"{tenant} layer {n} batch {b}: {stop}")
    log(state, f"done: {totals['deleted']} deleted, {totals['gone']} already gone")
    return 0


def id_pattern(t: str) -> str:
    """The Policy's idPattern of type t. Antares compiles it as ^(?:p)$ (antares-api policy.rs
    Rule::selects_id), the gateway unanchored (context-gateway pdp/id_match.rs): both anchors and
    the tail are written, so both select exactly the ids under the prefix."""
    return f"^urn:ngsi-ld:{t}:" + re.escape(DOMAIN) + ":.+$"


def policies(a) -> int:
    """Per space the temporary grant: deleteBatch only, for the cleanup account, one selector per
    minted type with the id pattern of that type under the prefix, valid until --until."""
    until = when(a.until)
    if not until:
        raise Stop("--until needs a UTC time, e.g. 2026-11-10T20:00:00Z")
    state = Path(a.state)
    state.mkdir(mode=0o700, parents=True, exist_ok=True)
    groups = load(a.list, a.endpoints, [])
    to = until.astimezone(dt.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    for project in sorted({g["project"] for g in groups.values()} if a.owner else ()):
        spaces = sorted(t for t, g in groups.items() if g["project"] == project)
        doc = {
            "apiVersion": "joinedcontext.com/v1alpha1", "kind": "ServiceAccount",
            "metadata": {"name": ACCOUNT, "namespace": project,
                         "title": {"en": f"Temporary cleanup account ({LABEL})"}},
            "spec": {
                "owner": {"user": a.owner},
                "purpose": f"temporary: deletes the {LABEL} copies; removed after the run",
                "roles": [{"role": "public", "scope": {"contextSpace": s}, "validity": {"notAfter": to}}
                          for s in spaces],
                "credentials": [{"kind": "oauth-client", "name": "main"}],
            },
        }
        (state / f"serviceaccount-{project}.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
        log(state, f"{project}: serviceaccount-{project}.json, space(s) {', '.join(spaces)}")
    for tenant, g in sorted(groups.items()):
        types = sorted({r["type"] for r in g["rows"].values()})
        doc = {
            "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Policy",
            "metadata": {"name": ACCOUNT, "namespace": g["project"],
                         "title": {"en": f"Temporary: the cleanup account deletes the copies minted under the wrong org domain ({LABEL})"}},
            "spec": {
                "contextSpaceRef": {"kind": "ContextSpace", "name": tenant},
                "assigner": "did:web:{orgDomain}",
                "assignee": {"kind": "serviceAccount", "id": ACCOUNT},
                "operations": ["deleteBatch"],
                "validity": {"to": to},
                "information": [{"entities": [
                    {"type": t, "idPattern": id_pattern(t)} for t in types]}],
            },
        }
        (state / f"policy-{tenant}.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
        log(state, f"{tenant}: policy-{tenant}.json, project {g['project']}, {len(types)} type(s)")
    return 0


def verify(a) -> int:
    state = Path(a.state)
    state.mkdir(mode=0o700, parents=True, exist_ok=True)
    now = dt.datetime.now(dt.UTC)
    left = {(t, i): when(e) for t, i, e in tsv(a.recount, 3)}
    live = defaultdict(int)
    for (t, _), e in left.items():
        if e is None or e > now:
            live[t] += 1
    expired = len(left) - sum(live.values())
    log(state, f"recount: {sum(live.values())} non-expired left {dict(live)}, {expired} expired left")
    history = tsv(a.history, 6)
    if not history:
        log(state, "history: none under the prefix; history step skipped")
        return 1 if live else 0
    sweep, stayed = [], []
    for t, i, *counts in history:
        e = left.get((t, i), "deleted")
        if e == "deleted":
            stayed.append("\t".join([t, i, *counts]))
        elif e is not None and e <= now:
            sweep.append(f"{t}\t{i}")
    (state / "history-for-sweep.tsv").write_text("".join(s + "\n" for s in sweep), encoding="utf-8")
    (state / "history-left-after-delete.tsv").write_text("".join(s + "\n" for s in stayed), encoding="utf-8")
    log(state, f"history: {len(history)} id(s); {len(sweep)} expired copies for the expiry sweep, "
               f"{len(stayed)} deleted ids whose history stayed")
    return 1 if live or stayed else 0


def configure(domain: str, account: str, label: str) -> None:
    """Another wrong-domain cleanup (T-6743: hel.fi copies in the Zilina spaces) reuses the tool."""
    global DOMAIN, ID, ACCOUNT, LABEL, REASON
    if not re.fullmatch(r"[a-z0-9]([a-z0-9.-]*[a-z0-9])?", domain):
        raise Stop(f"--domain {domain!r} is not a lowercase DNS name")
    if not re.fullmatch(r"[a-z0-9]([a-z0-9-]*[a-z0-9])?", account):
        raise Stop(f"--account {account!r} is not a lowercase resource name")
    DOMAIN, ACCOUNT, LABEL = domain, account, label
    ID = re.compile(r"urn:ngsi-ld:([A-Za-z]+):" + re.escape(DOMAIN) + r":\S+")
    REASON = f"cleanup: copies minted under the wrong org domain ({LABEL})"


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--domain", default=DEFAULTS[0], help="the wrong org domain the copies carry")
    p.add_argument("--account", default=DEFAULTS[1], help="the temporary ServiceAccount's name")
    p.add_argument("--label", default=DEFAULTS[2], help="the task named in titles and the delete reason")
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--list", required=True)
    r.add_argument("--endpoints", required=True)
    r.add_argument("--state", required=True)
    r.add_argument("--only", action="append", default=[])
    r.add_argument("--apply", action="store_true")
    r.add_argument("--gateway")
    r.add_argument("--token-file")
    r.add_argument("--token-url")
    r.add_argument("--client-secret-file")
    r.add_argument("--client-id")
    r.add_argument("--mem-cmd")
    r.add_argument("--max-batches", type=int)
    q = sub.add_parser("policies")
    q.add_argument("--list", required=True)
    q.add_argument("--endpoints", required=True)
    q.add_argument("--state", required=True)
    q.add_argument("--until", required=True)
    q.add_argument("--owner")
    v = sub.add_parser("verify")
    v.add_argument("--recount", required=True)
    v.add_argument("--history", required=True)
    v.add_argument("--state", required=True)
    a = p.parse_args(argv)
    if a.cmd == "run" and a.apply:
        if not (a.gateway and a.mem_cmd):
            p.error("--apply needs --gateway and --mem-cmd")
        if bool(a.token_file) == bool(a.client_secret_file) or bool(a.token_url) != bool(a.client_secret_file):
            p.error("--apply needs either --token-file or --token-url with --client-secret-file")
    if a.cmd == "run" and a.max_batches is not None and a.max_batches < 1:
        p.error("--max-batches needs a number >= 1")
    try:
        configure(a.domain, a.account, a.label)
        return {"run": run, "policies": policies, "verify": verify}[a.cmd](a)
    except Stop as e:
        log(Path(a.state) if Path(a.state).is_dir() else None, f"STOP: {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
