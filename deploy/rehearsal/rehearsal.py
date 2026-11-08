#!/usr/bin/env python3
"""Rehearsal of the one-binary version (`jc --role all`) on dev's and prod's config and files.

Subcommands (EXPORT.md has the bundle format, REHEARSAL.md the order):
  render   render the deployment repo's config for one environment (helmfile, read only)
  synth    write a synthetic export bundle from a render (until the deployers' export lands)
  realms   the export's realm config, users and credentials removed, for kaisatsu's import dir
  import   shako repositories, Antares entities and history, daiya dashboards into a running jc
  retain   delete Antares temporal instances older than 24 h from the rehearsal's database
  smoke    sign in as a test person and check every module; a report per check

Nothing here writes to dev or prod: every write goes to the jc given by --base, and the importer
refuses a --base that is not a loopback address. Secrets are read from files, never printed.
Standard library only.
"""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import html
import ipaddress
import json
import os
import re
import secrets
import subprocess
import threading
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from pathlib import Path

# Owner 2026-11-08: only the last day of Antares history is copied, and the rehearsal keeps only
# the last day (`retain`, hourly beside jc). Everything else is copied whole.
WINDOW = dt.timedelta(hours=24)
# `retain` runs hourly: an instance may outlive the window by up to one run.
SWEEP = dt.timedelta(hours=1)
# Realms of the old installation that kaisatsu does not take: Keycloak's own admin realm.
SKIPPED_REALMS = {"master"}
# What a realm export may hold that the rehearsal never takes: people and their secrets.
PEOPLE_KEYS = ("users", "federatedUsers")
MAX_BATCH_BYTES = 900_000  # below Antares' 4 MiB body cap and shako's request limit
MAX_BATCH_ENTITIES = 500
# jc's client that administers people in realm jc (bins/jc/src/setup.rs PORTAL_ADMIN_CLIENT).
PORTAL_ADMIN_CLIENT = "portal-reconciler"
IMPORT_WORKERS = 4  # parallel batch writes into the local broker
MAX_REPORTED = 200  # refused calls kept in the report; the rest are only counted


def utcnow() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0)


def iso(t: dt.datetime) -> str:
    return t.astimezone(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_time(s: str) -> dt.datetime:
    t = dt.datetime.fromisoformat(s.replace("Z", "+00:00"))
    if t.tzinfo is None:
        raise ValueError(f"time without a zone: {s}")
    return t


def cutoff_for(at: dt.datetime) -> dt.datetime:
    """The oldest observedAt taken: `at` (the export time) minus 24 hours."""
    return at - WINDOW


# --------------------------------------------------------------------------------------------
# HTTP


class Http:
    """A small client with a browser-like cookie jar (Secure and __Host- cookies kept on
    http://localhost, as browsers do); never follows redirects by itself."""

    def __init__(self, base: str):
        self.base = base.rstrip("/")
        self.jar: dict[str, tuple[str, str]] = {}

        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *a, **k):
                return None

        self.opener = urllib.request.build_opener(NoRedirect)

    def url(self, path: str) -> str:
        return path if path.startswith("http") else self.base + path

    def _store(self, headers):
        for raw in headers.get_all("Set-Cookie") or []:
            parts = [p.strip() for p in raw.split(";")]
            if "=" not in parts[0]:
                continue
            name, value = parts[0].split("=", 1)
            path, expired = "/", False
            for attr in parts[1:]:
                low = attr.lower()
                if low.startswith("path="):
                    path = attr[5:]
                if low == "max-age=0":
                    expired = True
            if expired or not value:
                self.jar.pop(name, None)
            else:
                self.jar[name] = (value, path)

    def _cookies(self, url: str) -> str:
        path = urllib.parse.urlparse(url).path or "/"
        return "; ".join(f"{n}={v}" for n, (v, p) in self.jar.items() if path.startswith(p))

    def call(self, method, path, body=None, headers=None, form=None, timeout=60):
        headers = dict(headers or {})
        url = self.url(path)
        data = None
        if form is not None:
            data = urllib.parse.urlencode(form).encode()
            headers.setdefault("Content-Type", "application/x-www-form-urlencoded")
        elif body is not None:
            data = body if isinstance(body, bytes) else json.dumps(body).encode()
            headers.setdefault("Content-Type", "application/json")
        if cookies := self._cookies(url):
            headers.setdefault("Cookie", cookies)
        if method not in ("GET", "HEAD") and "__Host-jc_csrf" in self.jar:
            headers.setdefault("x-csrf-token", self.jar["__Host-jc_csrf"][0])
        req = urllib.request.Request(url, data=data, method=method, headers=headers)
        try:
            with self.opener.open(req, timeout=timeout) as r:
                self._store(r.headers)
                return r.status, dict(r.headers), r.read()
        except urllib.error.HTTPError as e:
            if e.headers:
                self._store(e.headers)
            return e.code, dict(e.headers or {}), e.read() or b""

    def json(self, method, path, body=None, headers=None, form=None):
        status, hdrs, raw = self.call(method, path, body, headers, form)
        try:
            value = json.loads(raw) if raw else None
        except ValueError:
            value = raw.decode(errors="replace")[:300]
        return status, value


def loopback_only(base: str) -> None:
    host = urllib.parse.urlparse(base).hostname or ""
    if host == "localhost":
        return
    try:
        if ipaddress.ip_address(host).is_loopback:
            return
    except ValueError:
        pass
    raise SystemExit(f"refusing to write to {host}: the rehearsal imports into a local jc only")


def read_secret(state: Path, name: str) -> str:
    return (state / name).read_text().strip()


# --------------------------------------------------------------------------------------------
# render + synth (from the deployment repo's config, read only)

DEPLOY_ENV = {"dev": "dev", "prod": "smartcity"}


def cmd_render(a):
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, JUST_NO_DOTENV="true")
    hf = ["helmfile", "-f", "deployment/helmfile.yaml", "-e", a.env]
    steps = [
        (hf + ["write-values", "--selector", "name=keycloak-config",
               "--output-file-template", str(out / "{{ .Release.Name }}.values.yaml")], None),
        (hf + ["template", "--skip-deps", "--selector", "name=grafana-dashboards"], "grafana-dashboards.yaml"),
        (hf + ["template", "--skip-deps", "--selector", "name=gitea-bootstrap"], "gitea-bootstrap.yaml"),
        (hf + ["template", "--skip-deps", "--selector", "chart=sample-apps"], "gitea-seeds.yaml"),
    ]
    for argv, target in steps:
        r = subprocess.run(argv, cwd=a.deployment, env=env, capture_output=True, text=True)
        if r.returncode != 0:
            raise SystemExit(f"{' '.join(argv[:6])}: exit {r.returncode}\n{r.stderr[-2000:]}")
        if target:
            (out / target).write_text(r.stdout)
    print(f"rendered {a.env} into {out}")


def cmd_dashboards(a):
    """The Grafana files a render's dashboards chart provisions (platform models and, per project,
    folder, datasources and dashboard): what prod's and dev's Grafana load from files, not from
    their database, so an export bundle holds none of them."""
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    n = 0
    for doc in _yaml_docs(Path(a.render) / "grafana-dashboards.yaml"):
        for key, text in (doc.get("data") or {}).items():
            if key.endswith(".json"):
                (out / key).write_text(text)
                n += 1
    print(f"{n} Grafana files into {out}")


def _yaml_docs(path: Path):
    import yaml  # PyYAML is in the CI venv; only render/synth need it

    return [d for d in yaml.safe_load_all(path.read_text()) if d]


def _env_value(job_text: str, name: str):
    """The last value of an env var in the bootstrap Job: its main container comes after the
    init containers, which name the apps organization ORG too."""
    found = re.findall(rf"- name: {re.escape(name)}\n\s+value: \"([^\"]*)\"", job_text)
    return found[-1] if found else None


def layout2(seed: dict[str, str], org: str, repo: str):
    """Splits seed files (`a__b__c` keys) into repositories the way the forge bootstrap's layout
    2 does (components/gitea/charts/bootstrap/templates/job.yaml step 5d)."""
    repos: dict[tuple[str, str], dict[str, str]] = {(org, repo): {}}
    projects: dict[str, dict[str, str]] = {}
    for key, text in seed.items():
        m = re.fullmatch(r"projects__([a-z0-9-]+)__(.+)", key)
        if m:
            projects.setdefault(m.group(1), {})[m.group(2).replace("__", "/")] = text
        else:
            repos[(org, repo)][key.replace("__", "/")] = text
    for slug, files in projects.items():
        own = files.get("project.yaml", "")
        api = re.search(r"(?m)^apiVersion:\s*(\S+)", own)
        org_ref = re.search(r"(?m)^  organizationRef:\s*([A-Za-z0-9.-]+)", own)
        if not api or not org_ref:
            raise ValueError(f"project {slug}: project.yaml names no apiVersion or organizationRef")
        repos[(org, repo)][f"projects/{slug}.yaml"] = (
            f"apiVersion: {api.group(1)}\nkind: Project\nmetadata:\n  name: {slug}\n  namespace: org\n"
            f"spec:\n  organizationRef: {org_ref.group(1)}\n  repository:\n    name: {slug}\n  ref: main\n"
        )
        files[".jc/layout"] = "2\n"
        repos[(org, slug)] = files
    repos[(org, repo)][".jc/layout"] = "2\n"
    return repos


def synth_entities(repos, now: dt.datetime):
    """A few entities per space, of the classes its data models name, with history inside the
    window and one instance outside it (which the importer must refuse)."""
    entities, temporal = [], []
    for (_, name), files in sorted(repos.items()):
        for path, text in sorted(files.items()):
            m = re.fullmatch(r"spaces/([a-z0-9-]+)/datamodels/[^/]+\.linkml\.yaml", path)
            if not m:
                continue
            tenant = f"{name}-{m.group(1)}"
            classes = re.findall(r"(?m)^  ([A-Z][A-Za-z0-9]+):\s*$", text.split("\nclasses:", 1)[-1])
            for cls in classes[:3]:
                for n in range(2):
                    eid = f"urn:ngsi-ld:{cls}:rehearsal-{n}"
                    entities.append({"tenant": tenant, "entity": {
                        "id": eid, "type": cls,
                        "name": {"type": "Property", "value": f"{cls} {n}"},
                        "observationCount": {"type": "Property", "value": 3,
                                              "observedAt": iso(now - dt.timedelta(hours=1))},
                    }})
                    for hours in (1, 23, 25):
                        temporal.append({"tenant": tenant, "id": eid, "type": cls,
                                         "attr": "observationCount", "instance": {
                                             "type": "Property", "value": hours,
                                             "observedAt": iso(now - dt.timedelta(hours=hours))}})
    return entities, temporal


def cmd_synth(a):
    render, out = Path(a.render), Path(a.out)
    now = utcnow()
    out.mkdir(parents=True, exist_ok=True)
    # realm config: keycloak-config-cli's inline realms
    import yaml

    values = yaml.safe_load((render / "keycloak-config.values.yaml").read_text())
    (out / "keycloak").mkdir(exist_ok=True)
    for name, block in values["config"].items():
        realm = strip_people(block["inline"])
        (out / "keycloak" / f"realm-{name}.json").write_text(json.dumps(realm, indent=1))
    # forge: the bootstrap's organization files and every project seed, cut as layout 2
    job = (render / "gitea-bootstrap.yaml").read_text()
    org, repo = _env_value(job, "ORG") or "joinedcontext", _env_value(job, "REPO") or "configuration"
    apps_owner = _env_value(job, "APPS_OWNER") or org
    apps_project = _env_value(job, "APPS_PROJECT") or "helsinki"
    seed: dict[str, str] = {}
    apps: dict[str, dict[str, str]] = {}
    for doc in _yaml_docs(render / "gitea-bootstrap.yaml") + _yaml_docs(render / "gitea-seeds.yaml"):
        if doc.get("kind") != "ConfigMap":
            continue
        n, data = doc["metadata"]["name"], doc.get("data") or {}
        if n.startswith("gitea-bootstrap-seed") or n.startswith("gitea-seed"):
            seed.update(data)
        elif n.startswith("gitea-sample-apps-"):
            apps[n.removeprefix("gitea-sample-apps-")] = {k.replace("__", "/"): v for k, v in data.items()}
    repos = layout2(seed, org, repo)
    for app, files in apps.items():
        repos[(apps_owner, f"{apps_project}_{app}")] = files
    gitea = out / "gitea"
    listing = []
    for (owner, name), files in sorted(repos.items()):
        listing.append({"owner": owner, "name": name, "defaultBranch": "main", "private": True,
                        "description": ""})
        for path, text in files.items():
            p = gitea / "files" / owner / name / path
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(text)
    (gitea / "repos.json").write_text(json.dumps(listing, indent=1))
    # dashboards: the JSON of every dashboard ConfigMap
    (out / "grafana" / "dashboards").mkdir(parents=True, exist_ok=True)
    for doc in _yaml_docs(render / "grafana-dashboards.yaml"):
        for key, text in (doc.get("data") or {}).items():
            if key.endswith(".json"):
                (out / "grafana" / "dashboards" / key).write_text(text)
    # broker: synthetic entities of the seeded data models
    entities, temporal = synth_entities(repos, now)
    (out / "antares").mkdir(exist_ok=True)
    (out / "antares" / "entities.jsonl").write_text("".join(json.dumps(e) + "\n" for e in entities))
    (out / "antares" / "temporal.jsonl").write_text("".join(json.dumps(t) + "\n" for t in temporal))
    manifest = {"env": a.env, "exportedAt": iso(now), "synthetic": True,
                "source": "joinedcontext-deployment config (synth)", "repos": len(listing),
                "entities": len(entities), "temporalInstances": len(temporal)}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=1))
    print(json.dumps(manifest))


# --------------------------------------------------------------------------------------------
# realms


TESTER = "rehearsal-tester"


def strip_people(realm: dict) -> dict:
    """The realm without people: no person, no credential, no session. Clients, scopes, roles,
    groups, mappers and flows stay, and so do the service-account users that carry a client's
    role bindings (a client, not a person), without any credential."""
    out = {k: v for k, v in realm.items() if k not in PEOPLE_KEYS}
    accounts = []
    for u in realm.get("users") or []:
        if u.get("serviceAccountClientId"):
            accounts.append({k: v for k, v in u.items() if k not in ("credentials", "email", "firstName", "lastName")})
    if accounts:
        out["users"] = accounts
    return out


def add_tester(realm: dict) -> dict:
    """The rehearsal's own test person, its password a secret variable up.sh fills."""
    out = dict(realm)
    tester = {"username": TESTER, "enabled": True, "email": f"{TESTER}@rehearsal.invalid",
              "emailVerified": True, "firstName": "Rehearsal", "lastName": "Tester",
              "credentials": [{"type": "password", "value": "$(env:REHEARSAL_TESTER_PASSWORD)", "temporary": False}]}
    out["users"] = list(out.get("users") or []) + [tester]
    return out


UNKNOWN_MAPPER = re.compile(
    r"\$\.(clientScopes|clients)\[(\d+)\]\.protocolMappers\[(\d+)\]\.protocolMapper: unknown mapper kind '([^']+)'")


UNMAPPABLE_FLOW = re.compile(r"\$\.authenticationFlows\[(\d+)\]\.alias: unmappable authentication flow '([^']*)'")
UNKNOWN_AUTHENTICATOR = re.compile(
    r"\$\.authenticationFlows\[(\d+)\]\.authenticationExecutions\[\d+\]\.authenticator: unknown authenticator '([^']*)'")
FLOW_BINDINGS = ("browserFlow", "directGrantFlow", "registrationFlow", "resetCredentialsFlow",
                 "clientAuthenticationFlow", "dockerAuthenticationFlow", "firstBrokerLoginFlow")


def _flow_in_use(realm: dict, alias: str) -> bool:
    """Bound to the realm or a client. A subflow reference does not count: kaisatsu maps each
    flow it knows by its alias, whole, to its own state machine and reads no subflow."""
    serves = {
        # The docker registry flow serves only docker-v2 clients.
        "dockerAuthenticationFlow": any(c.get("protocol") == "docker-v2" for c in realm.get("clients", [])),
        # First broker login runs only for a login broker; a `kubernetes` provider is the issuer of
        # workload client assertions (federated-jwt) and signs in no person.
        "registrationFlow": bool(realm.get("registrationAllowed")),
        "firstBrokerLoginFlow": any(i.get("providerId") != "kubernetes" for i in realm.get("identityProviders", [])),
    }
    if any(realm.get(b) == alias for b in FLOW_BINDINGS if serves.get(b, True)):
        return True
    if any(alias in (i.get("firstBrokerLoginFlowAlias"), i.get("postBrokerLoginFlowAlias"))
           for i in realm.get("identityProviders", [])):
        return True
    return any(alias in (c.get("authenticationFlowBindingOverrides") or {}).values() for c in realm.get("clients", []))


def drop_refused_mappers(realm: dict, check) -> tuple[dict, list]:
    """Asks kaisatsu (`check(realm) -> error text or ""`) what it refuses and leaves out only what
    a live Keycloak export carries beside the realm's own config: protocol mappers of a kind
    kaisatsu does not know (Keycloak's built-in scopes), and built-in flows it cannot map, or that hold an
    authenticator it does not know, that no realm or client binding names. Each is named in the returned list. Any other refusal,
    or a refused flow in use, stops: it is a gap, not something to drop."""
    dropped = []
    for _ in range(500):
        err = check(realm)
        if not err:
            return realm, dropped
        if m := UNKNOWN_MAPPER.search(err):
            where, i, mi, kind = m.group(1), int(m.group(2)), int(m.group(3)), m.group(4)
            owner = realm[where][i]
            mapper = owner["protocolMappers"].pop(mi)
            dropped.append({"in": f"{where}/{owner.get('name') or owner.get('clientId')}",
                            "mapper": mapper.get("name"), "kind": kind})
            continue
        if (m := UNMAPPABLE_FLOW.search(err)) and not _flow_in_use(realm, m.group(2)):
            realm["authenticationFlows"].pop(int(m.group(1)))
            dropped.append({"in": "authenticationFlows", "flow": m.group(2), "kind": "unmapped unused flow"})
            continue
        if m := UNKNOWN_AUTHENTICATOR.search(err):
            flow = realm["authenticationFlows"][int(m.group(1))]
            alias, auth = flow.get("alias", ""), m.group(2)
            if not _flow_in_use(realm, alias):
                realm["authenticationFlows"].pop(int(m.group(1)))
                for b in ("dockerAuthenticationFlow", "firstBrokerLoginFlow", "registrationFlow"):
                    if realm.get(b) == alias:
                        del realm[b]
                dropped.append({"in": "authenticationFlows", "flow": alias,
                                "kind": f"unused flow with unknown authenticator {auth}"})
                continue
            # A bound flow keeps every step a client could need: only an alternative (or a
            # disabled step) of an authenticator no client asks for goes.
            execs = flow.get("authenticationExecutions") or []
            step = next((e for e in execs if e.get("authenticator") == auth), None)
            used = any(c.get("clientAuthenticatorType") == auth for c in realm.get("clients", []))
            if step and step.get("requirement") in ("ALTERNATIVE", "DISABLED") and not used:
                execs.remove(step)
                dropped.append({"in": f"authenticationFlows/{alias}", "authenticator": auth,
                                "kind": f"{step.get('requirement').lower()} step no client uses"})
                continue
        raise SystemExit(f"kaisatsu refuses the realm: {err.strip()[:500]}")
    raise SystemExit("kaisatsu still refuses the realm after 500 items left out")


def kaisatsu_check(kaisatsu_bin: str, tmp: Path):
    def check(realm: dict) -> str:
        tmp.write_text(json.dumps(realm))
        r = subprocess.run([kaisatsu_bin, "check", "-f", str(tmp)], capture_output=True, text=True)
        return "" if r.returncode == 0 else (r.stderr or r.stdout)
    return check


def cmd_realms(a):
    src, dst = Path(a.export) / "keycloak", Path(a.out)
    dst.mkdir(parents=True, exist_ok=True)
    taken = []
    for f in sorted(src.glob("realm-*.json")):
        realm = json.loads(f.read_text())
        name = realm.get("realm", "")
        if name in SKIPPED_REALMS:
            continue
        clean = add_tester(strip_people(realm))
        if a.kaisatsu_bin:
            tmp = dst.parent / f".check-{name}.json"
            clean, dropped = drop_refused_mappers(clean, kaisatsu_check(a.kaisatsu_bin, tmp))
            tmp.unlink(missing_ok=True)
            (dst.parent / f"realm-{name}-left-out.json").write_text(json.dumps(dropped, indent=1))
            if dropped:
                print(f"realm {name}: {len(dropped)} parts kaisatsu refuses left out "
                      f"(realm-{name}-left-out.json): {sorted({d['kind'] for d in dropped})}")
        (dst / f"{name}.json").write_text(json.dumps(clean, indent=1))
        taken.append(name)
        # The secret variables the realm names ($(env:NAME)): up.sh makes a value for each.
        names = sorted(set(re.findall(r"\$\((?:env:)?([A-Z][A-Z0-9_]*)\)", json.dumps(clean))))
        (dst.parent / "realm-vars.txt").write_text("".join(n + "\n" for n in names))
    if len(taken) > 1:
        # jc's kaisatsu import dir merges every file into one realm.
        raise SystemExit(f"more than one realm to import: {taken}")
    print(f"realm config for kaisatsu: {taken}")


# --------------------------------------------------------------------------------------------
# import


def git_blob_sha(data: bytes) -> str:
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()


CKAN_URL = re.compile(rb"^(\s+url:\s*)\S+", re.M)
# The catalogue name every CkanInstance gets in a rehearsal. jc-core wants an https URL (EP-67);
# up.sh routes this name to the rehearsal's own dataroom (JC_PORTAL_CKAN_URL -> _API_URL), and
# `.invalid` never resolves should anything call it directly.
REHEARSAL_CATALOGUE = b"https://catalogue.rehearsal.invalid"


def fence_writes(data: bytes) -> bytes:
    """A CkanInstance is the one manifest the Portal writes to an outside host with (its API
    token publishes datasets there): its `spec.url` is turned to the rehearsal's catalogue name,
    so a rehearsal never publishes into the catalogue of the environment it was copied from."""
    if not re.search(rb"^kind:\s*CkanInstance\s*$", data, re.M):
        return data
    return CKAN_URL.sub(lambda m: m.group(1) + REHEARSAL_CATALOGUE, data)


def import_shako(h: Http, token: str, export: Path, report: dict):
    auth = {"Authorization": f"token {token}"}
    listing = json.loads((export / "gitea" / "repos.json").read_text())
    rows = []
    for r in listing:
        owner, name, branch = r["owner"], r["name"], r.get("defaultBranch") or "main"
        root = export / "gitea" / "files" / owner / name
        files = {}
        for p in sorted(root.rglob("*")):
            if p.is_file() and not p.is_symlink():
                data = p.read_bytes()
                files[p.relative_to(root).as_posix()] = fenced = fence_writes(data)
                if fenced != data:
                    report.setdefault("fenced", []).append(f"{owner}/{name}/{p.relative_to(root).as_posix()}")
        row = {"repo": f"{owner}/{name}", "files": len(files)}
        st, _ = h.json("GET", f"/git/api/v1/projects/{owner}/repositories/{name}", headers=auth)
        if st == 404:
            st, body = h.json("POST", f"/git/api/v1/projects/{owner}/repositories", headers=auth,
                              body={"name": name, "private": bool(r.get("private", True)),
                                    "default_branch": branch,
                                    "description": r.get("description") or ""})
            if st not in (200, 201):
                row.update(result="refused", call=f"POST /git/api/v1/projects/{owner}/repositories",
                           status=st, detail=body)
                rows.append(row)
                continue
            row["created"] = True
        held = {}
        for page in range(1, 1000):
            st, tree = h.json("GET", f"/git/api/v1/repos/{owner}/{name}/git/trees/{branch}"
                                     f"?recursive=true&per_page=1000&page={page}", headers=auth)
            if st != 200 or not isinstance(tree, dict):
                break
            held.update({e["path"]: e["sha"] for e in tree.get("tree", []) if e.get("type") == "blob"})
            if not tree.get("truncated"):
                break
        ops = []
        for path, data in files.items():
            sha = git_blob_sha(data)
            if held.get(path) == sha:
                continue
            op = {"operation": "update" if path in held else "create", "path": path,
                  "content": base64.b64encode(data).decode()}
            if path in held:
                op["sha"] = held[path]
            ops.append(op)
        for path in sorted(set(held) - set(files)):
            ops.append({"operation": "delete", "path": path, "sha": held[path]})
        commits, failed = 0, None
        batch, size = [], 0
        for op in ops + [None]:
            if op is not None and (not batch or size + len(op.get("content", "")) < MAX_BATCH_BYTES):
                batch.append(op)
                size += len(op.get("content", ""))
                continue
            if batch:
                st, body = h.json("POST", f"/git/api/v1/projects/{owner}/repositories/{name}/contents",
                                  headers=auth, body={"branch": branch, "files": batch,
                                                      "message": "rehearsal import: current files"})
                if st not in (200, 201):
                    failed = {"call": f"POST /git/api/v1/projects/{owner}/repositories/{name}/contents",
                              "status": st, "detail": body}
                    break
                commits += 1
            batch, size = ([op], len(op.get("content", ""))) if op else ([], 0)
        row.update(changed=len(ops), commits=commits)
        if failed:
            row.update(result="refused", **failed)
        else:
            row["result"] = "imported" if ops else "unchanged"
        rows.append(row)
    report["shako"] = rows


def split_by_cutoff(lines, cutoff: dt.datetime):
    """Temporal instances observed at or after `cutoff` are taken; older ones and ones without an
    observedAt are refused, each with its reason."""
    kept, refused = [], []
    for t in lines:
        at = t.get("instance", {}).get("observedAt")
        if not at:
            refused.append({**_key(t), "reason": "no observedAt"})
            continue
        try:
            when = parse_time(at)
        except ValueError:
            refused.append({**_key(t), "reason": f"bad observedAt {at!r}"})
            continue
        if when < cutoff:
            refused.append({**_key(t), "observedAt": at, "reason": f"older than the cut-off {iso(cutoff)}"})
        else:
            kept.append(t)
    return kept, refused


def _key(t):
    return {"tenant": t.get("tenant"), "id": t.get("id"), "attr": t.get("attr")}


def _jsonl(path: Path):
    """The lines of a JSONL file, one at a time: a bundle's entities run to gigabytes."""
    if not path.exists():
        return
    with path.open(encoding="utf-8") as f:
        for line in f:
            if line.strip():
                yield json.loads(line)


def _chunks(items, limit=MAX_BATCH_BYTES, count=MAX_BATCH_ENTITIES):
    batch, size = [], 0
    for it in items:
        n = len(json.dumps(it))
        if batch and (size + n > limit or len(batch) >= count):
            yield batch
            batch, size = [], 0
        batch.append(it)
        size += n
    if batch:
        yield batch


def _tenant_batches(lines):
    """(tenant, batch) from a stream of {tenant, entity} lines; the file need not be sorted."""
    def runs():
        tenant, run = None, []
        for e in lines:
            if e["tenant"] != tenant and run:
                yield tenant, run
                run = []
            tenant = e["tenant"]
            run.append(e["entity"])
            if len(run) >= MAX_BATCH_ENTITIES * 4:
                yield tenant, run
                run = []
        if run:
            yield tenant, run
    for tenant, run in runs():
        for batch in _chunks(run):
            yield tenant, batch


def import_antares(h: Http, export: Path, cutoff: dt.datetime, report: dict):
    base = "/broker/ngsi-ld/v1"
    out = {"entities": 0, "entityCalls": 0, "refusedCalls": [], "refusedCallCount": 0,
           "temporal": 0, "temporalRefused": [], "temporalRefusedCount": 0}
    lock = threading.Lock()

    def refused(row):
        with lock:
            out["refusedCallCount"] += 1
            if len(out["refusedCalls"]) < MAX_REPORTED:
                out["refusedCalls"].append(row)

    def upsert(job):
        tenant, batch = job
        st, body = h.json("POST", f"{base}/entityOperations/upsert?options=update", body=batch,
                          headers={"NGSILD-Tenant": tenant})
        ok = len(batch) if st in (200, 201, 204) else \
            len(body.get("success", [])) if st == 207 and isinstance(body, dict) else 0
        with lock:
            out["entities"] += ok
            out["entityCalls"] += 1
            if out["entityCalls"] % 200 == 0:
                print(f"antares: {out['entities']} entities after {out['entityCalls']} calls", file=sys.stderr, flush=True)
        if ok < len(batch):
            refused({"call": f"POST {base}/entityOperations/upsert", "tenant": tenant, "status": st,
                     "detail": (body.get("errors", [])[:5] if st == 207 and isinstance(body, dict) else body)})

    def temporal(job):
        (tenant, eid), ent = job
        st, body = h.json("POST", f"{base}/temporal/entities", body=ent, headers={"NGSILD-Tenant": tenant})
        if st in (200, 201, 204):
            with lock:
                out["temporal"] += sum(len(v) for k, v in ent.items() if k not in ("id", "type"))
        else:
            refused({"call": f"POST {base}/temporal/entities", "tenant": tenant, "id": eid,
                     "status": st, "detail": body})

    with ThreadPoolExecutor(IMPORT_WORKERS) as pool:
        # bounded: at most a few batches in flight, never the whole file in memory
        pending = set()
        for job in _tenant_batches(_jsonl(export / "antares" / "entities.jsonl")):
            pending.add(pool.submit(upsert, job))
            if len(pending) >= IMPORT_WORKERS * 2:
                done, pending = wait(pending, return_when=FIRST_COMPLETED)
                for f in done:
                    f.result()
        for f in pending:
            f.result()
        kept, refused_old = split_by_cutoff(_jsonl(export / "antares" / "temporal.jsonl"), cutoff)
        out["temporalRefusedCount"] = len(refused_old)
        out["temporalRefused"] = refused_old[:MAX_REPORTED]
        grouped: dict[tuple, dict] = {}
        for t in kept:
            ent = grouped.setdefault((t["tenant"], t["id"]), {"id": t["id"], "type": t["type"]})
            ent.setdefault(t["attr"], []).append(t["instance"])
        for f in [pool.submit(temporal, job) for job in sorted(grouped.items())]:
            f.result()
    report["antares"] = out


def _daiya_kind(name: str) -> str:
    """Grafana provisioning files of the deployment: `<project>.folder.json`,
    `<project>.datasource.<name>.json`, `<project>.dashboard.json` (an /api/dashboards/db body)
    and plain dashboard models (`portal.json`)."""
    for kind in ("folder", "datasource", "dashboard"):
        if f".{kind}." in name or name.endswith(f".{kind}.json"):
            return kind
    return "model"


def rehearsal_endpoint_url(url: str, base: str) -> str:
    """A datasource's Endpoint URL (`<gateway>/api/endpoint/<slug>[/...]`) on the rehearsal's own
    gateway: the same slug, the rehearsal's base. Any other URL is left as it is."""
    head, sep, rest = url.partition("/api/endpoint/")
    return f"{base.rstrip('/')}/api/endpoint/{rest}" if sep and head and rest else url


def daiya_org(h: Http, auth: dict, project: str) -> int:
    """The project's daiya organization, made when missing (provision.sh's POST /orgs, 409 ->
    GET /orgs/name/{project}); the Portal's provisioning loop may have made it first."""
    st, body = h.json("POST", "/dashboards/api/orgs", headers=auth, body={"name": project})
    if st == 409:
        st, body = h.json("GET", f"/dashboards/api/orgs/name/{urllib.parse.quote(project)}", headers=auth)
    org = (body or {}).get("orgId") or (body or {}).get("id") if isinstance(body, dict) else None
    if st not in (200, 201) or not org:
        raise RuntimeError(f"organization {project}: {st} {str(body)[:200]}")
    return int(org)


def import_daiya(h: Http, state: Path, export: Path, daiya_bin: str | None, report: dict,
                 dashboards: Path | None = None):
    """The deployment's Grafana provisioning into daiya, as provision.sh does it: each project's
    files in its own organization (folder, datasources on the rehearsal's Endpoints, dashboard),
    the plain dashboard models in the main organization."""
    pw = read_secret(state, "dashboards-password")
    auth = {"Authorization": "Basic " + base64.b64encode(f"portal:{pw}".encode()).decode()}
    order = {"folder": 0, "datasource": 1, "dashboard": 2, "model": 2}
    src = dashboards or export / "grafana" / "dashboards"
    files = sorted(src.glob("*.json"), key=lambda f: (order[_daiya_kind(f.name)], f.name))
    rows, orgs = [], {}
    for f in files:
        kind = _daiya_kind(f.name)
        row = {"file": f.name, "kind": kind}
        hdrs = dict(auth)
        if kind != "model":
            project = f.name.split(".", 1)[0]
            try:
                if project not in orgs:
                    orgs[project] = daiya_org(h, auth, project)
            except RuntimeError as e:
                rows.append({**row, "result": "refused", "reason": str(e)})
                continue
            hdrs["X-Grafana-Org-Id"] = str(orgs[project])
            row.update(project=project, org=orgs[project])
        try:
            doc = json.loads(f.read_text())
        except ValueError as e:
            rows.append({**row, "result": "refused", "reason": f"not JSON: {e}"})
            continue
        if kind == "folder":
            call, body = "POST /dashboards/api/folders", {"uid": doc.get("uid"), "title": doc.get("title")}
            row.update(uid=doc.get("uid"), title=doc.get("title"))
        elif kind == "datasource":
            mapped = rehearsal_endpoint_url(doc.get("url", ""), h.base)
            doc = {**doc, "url": mapped}
            call, body = "POST /dashboards/api/datasources", doc
            row.update(uid=doc.get("uid"), title=doc.get("name"), url=mapped)
        else:
            dash = doc.get("dashboard", doc) if kind == "dashboard" else doc
            body = {"dashboard": {k: v for k, v in dash.items() if k != "id"}, "overwrite": True,
                    "message": "rehearsal import"}
            if kind == "dashboard" and doc.get("folderUid"):
                body["folderUid"] = doc["folderUid"]
            call = "POST /dashboards/api/dashboards/db"
            row.update(uid=dash.get("uid"), title=dash.get("title"))
            if daiya_bin:
                tmp = state.parent / "daiya-check.json"
                tmp.write_text(json.dumps(dash))
                r = subprocess.run([daiya_bin, "check", "-f", str(tmp), "--format", "json"],
                                   capture_output=True, text=True)
                try:
                    check = json.loads(r.stdout)
                    d = (check.get("dashboards") or [{}])[0]
                    row["check"] = {"exit": r.returncode,
                                    "findings": sorted({f"{x.get('verdict')}:{x.get('code')}"
                                                        for x in d.get("findings", [])
                                                        if str(x.get("verdict")).lower() not in ("supports", "supported")})}
                except ValueError:
                    row["check"] = {"exit": r.returncode, "stderr": r.stderr[-300:]}
        st, answer = h.json("POST", call.split(" ", 1)[1], headers=hdrs, body=body)
        if st == 409 and kind == "datasource":  # provision.sh: an existing one is replaced
            st, answer = h.json("PUT", f"/dashboards/api/datasources/uid/{urllib.parse.quote(doc['uid'])}",
                                headers=hdrs, body=body)
        if st in (200, 201):
            changed = bool(row.get("check", {}).get("findings"))
            row.update(result="changed" if changed else "imported", status=st)
        elif st in (409, 412) and kind == "folder":
            row.update(result="unchanged", status=st)
        else:
            row.update(result="refused", call=call, status=st, detail=answer)
        rows.append(row)
    report["daiya"] = rows


def bundle_settled(export: Path) -> str:
    """Why the bundle is still being written, or "" when it is not: manifest.json is written last,
    so a data file newer than it belongs to an export in progress. An exporter's own work
    directories (`.parts`) are not read."""
    manifest = export / "manifest.json"
    if not manifest.exists():
        return "no manifest.json"
    at = manifest.stat().st_mtime
    newer = sorted(str(p.relative_to(export)) for p in export.rglob("*")
                   if p.is_file() and ".parts" not in p.parts and p.stat().st_mtime > at)
    return f"written after manifest.json: {newer[:3]}" if newer else ""


def cmd_import(a):
    loopback_only(a.base)
    export, state = Path(a.export), Path(a.state)
    if why := bundle_settled(export):
        raise SystemExit(f"the bundle is not settled ({why}): import after the exporter finishes")
    now = parse_time(a.now) if a.now else utcnow()
    manifest = json.loads((export / "manifest.json").read_text())
    # Only the last day: observed at most 24 h before the export, and at most 24 h before now (the
    # rehearsal's rolling retention would delete it at once).
    exported = parse_time(manifest["exportedAt"]) if manifest.get("exportedAt") else now
    cutoff = max(cutoff_for(exported), cutoff_for(now))
    report = {"env": manifest.get("env"), "export": str(export), "exportedAt": manifest.get("exportedAt"),
              "synthetic": manifest.get("synthetic", False), "importedAt": iso(now), "cutoff": iso(cutoff),
              "rule": f"no temporal instance observed before {iso(cutoff)} (the later of export time and now, - 24 h) is imported; "
                      "config, files, current entity state and dashboards are copied whole"}
    h = Http(a.base)
    steps = a.only.split(",") if a.only else ["shako", "antares", "daiya"]
    if "shako" in steps:
        import_shako(h, read_secret(state, "shako-token"), export, report)
    if "antares" in steps:
        import_antares(h, export, cutoff, report)
    if "daiya" in steps:
        import_daiya(h, state, export, a.daiya_bin, report, Path(a.dashboards) if a.dashboards else None)
    Path(a.report).write_text(json.dumps(report, indent=1))
    summary = {
        "cutoff": report["cutoff"],
        "shako": _count(report.get("shako", []), "result"),
        "antares": {k: (len(v) if isinstance(v, list) else v) for k, v in report.get("antares", {}).items()},
        "daiya": _count(report.get("daiya", []), "result"),
    }
    print(json.dumps(summary))


def _count(rows, key):
    out: dict[str, int] = {}
    for r in rows:
        out[r.get(key, "?")] = out.get(r.get(key, "?"), 0) + 1
    return out


# --------------------------------------------------------------------------------------------
# retain: Antares temporal instances older than 24 h, in the rehearsal's own database only

# Antares' own retention (ANTARES_TEMPORAL_RETENTION_DAYS, whole days) drops a weekly partition
# only once all of it is older, so a 25 h instance in this week's partition stays. This runs its
# DEFAULT-partition purge statement on the parents instead: the same observed_at condition, audit
# rows kept, service mode for the forced row-level security. Current state (entities) is never
# touched: it lives in other tables.
HISTORY_TABLES = ("attr_instances", "attr_rows")


def retain_sql(cutoff: dt.datetime) -> str:
    stmts = ["BEGIN;", "SELECT set_config('antares.service', 'on', true) \\g /dev/null"]
    for table in HISTORY_TABLES:
        stmts.append(
            f"SELECT to_regclass('public.{table}') IS NOT NULL AS has \\gset\n"
            f"\\if :has\n"
            f"PREPARE d_{table}(timestamptz) AS DELETE FROM public.{table} WHERE observed_at < $1 "
            f"AND NOT coalesce(provenance @> '{{\"audit\": true}}', false);\n"
            f"EXECUTE d_{table}('{iso(cutoff)}');\n"
            f"\\endif")
    stmts.append("COMMIT;")
    return "\n".join(stmts) + "\n"


def retain_once(a, now: dt.datetime) -> dict:
    cutoff = cutoff_for(now)
    env = dict(os.environ, PGPASSWORD=Path(a.password_file).read_text().strip())
    psql = ["psql", "-AX", "-v", "ON_ERROR_STOP=1", "-h", a.host, "-p", str(a.port), "-U", a.user, "-d", a.db]
    r = subprocess.run(psql, input=retain_sql(cutoff), env=env, capture_output=True, text=True)
    deleted = [int(x) for x in re.findall(r"^DELETE (\d+)$", r.stdout, re.M)]
    return {"at": iso(now), "cutoff": iso(cutoff), "deleted": sum(deleted),
            **({} if r.returncode == 0 else {"failed": r.stderr.strip()[-300:]})}


def old_history_sql(cutoff: dt.datetime) -> str:
    """Counts the history rows older than `cutoff` that retain would delete (audit rows aside)."""
    stmts = ["BEGIN;", "SELECT set_config('antares.service', 'on', true) \\g /dev/null"]
    for table in HISTORY_TABLES:
        stmts.append(
            f"SELECT to_regclass('public.{table}') IS NOT NULL AS has \\gset\n"
            f"\\if :has\n"
            f"SELECT 'old {table} ' || count(*) FROM public.{table} WHERE observed_at < '{iso(cutoff)}' "
            f"AND NOT coalesce(provenance @> '{{\"audit\": true}}', false);\n"
            f"\\endif")
    stmts.append("COMMIT;")
    return "\n".join(stmts) + "\n"


def old_history(host, port, user, db, password_file, cutoff: dt.datetime) -> int:
    """History rows older than `cutoff` in the rehearsal's Antares database; raises when it
    cannot read them (a check that cannot look must not pass)."""
    env = dict(os.environ, PGPASSWORD=Path(password_file).read_text().strip())
    psql = ["psql", "-AXt", "-v", "ON_ERROR_STOP=1", "-h", host, "-p", str(port), "-U", user, "-d", db]
    r = subprocess.run(psql, input=old_history_sql(cutoff), env=env, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip()[-300:])
    return sum(int(x) for x in re.findall(r"^old \w+ (\d+)$", r.stdout, re.M))


def cmd_retain(a):
    """Deletes temporal instances older than 24 h now and then every `--every` seconds."""
    while True:
        now = parse_time(a.now) if a.now else utcnow()
        print(json.dumps(retain_once(a, now)), flush=True)
        if not a.every:
            return 0
        time.sleep(a.every)


# --------------------------------------------------------------------------------------------
# smoke


def _hidden(page: str, name: str) -> str:
    m = re.search(rf'<input[^>]*name="{name}"[^>]*>', page)
    v = re.search(r'value="([^"]*)"', m.group(0)) if m else None
    if not v:
        raise RuntimeError(f"no {name} on the login page")
    return html.unescape(v.group(1))


def follow(h: Http, url: str, hops=12):
    for _ in range(hops):
        st, hdrs, body = h.call("GET", url)
        loc = hdrs.get("Location") or hdrs.get("location")
        if st in (301, 302, 303, 307, 308) and loc:
            url = urllib.parse.urljoin(h.url(url), loc)
            continue
        return url, st, body
    raise RuntimeError(f"too many redirects from {url}")


def sign_in(h: Http, username: str, password: str):
    login_url, st, page = follow(h, "/api/v1/auth/login")
    if st != 200:
        return False, f"GET /api/v1/auth/login -> {st} at {login_url}"
    text = page.decode(errors="replace")
    form = {"csrf_token": _hidden(text, "csrf_token"), "session_code": _hidden(text, "session_code"),
            "username": username, "password": password}
    st, hdrs, body = h.call("POST", login_url, form=form)
    loc = hdrs.get("Location") or hdrs.get("location") or ""
    if st not in (302, 303) or "code=" not in loc:
        return False, f"POST {urllib.parse.urlparse(login_url).path} -> {st} {body[:200]!r}"
    _, st, _ = follow(h, urllib.parse.urljoin(login_url, loc))
    return st < 400, f"callback -> {st}"


def admin_token(h: Http, state: Path) -> str:
    st, body = h.json("POST", "/id/realms/jc/protocol/openid-connect/token", form={
        "grant_type": "client_credentials", "client_id": PORTAL_ADMIN_CLIENT,
        "client_secret": read_secret(state, "portal-admin-client-secret")})
    if st != 200:
        raise RuntimeError(f"{PORTAL_ADMIN_CLIENT} token: {st} {body}")
    return body["access_token"]


def make_person(h: Http, token: str, realm: str, username: str, groups=()):
    """A test person with a random password (kept in memory only): create, then set the password."""
    auth = {"Authorization": f"Bearer {token}"}
    # The random part carries the entropy; the suffix meets a realm password policy (digit, cases, symbol).
    password = secrets.token_urlsafe(18) + "-7aZ"
    st, body = h.json("POST", f"/id/admin/realms/{realm}/users", headers=auth, body={
        "username": username, "enabled": True, "email": f"{username}@rehearsal.invalid",
        "emailVerified": True, "firstName": "Rehearsal", "lastName": username, "groups": list(groups)})
    if st not in (201, 409):
        raise RuntimeError(f"create {username} in {realm}: {st} {body}")
    st, users = h.json("GET", f"/id/admin/realms/{realm}/users?username={username}&exact=true", headers=auth)
    uid = users[0]["id"]
    st, body = h.json("PUT", f"/id/admin/realms/{realm}/users/{uid}/reset-password", headers=auth,
                      body={"type": "password", "value": password, "temporary": False})
    if st not in (200, 204):
        raise RuntimeError(f"password of {username}: {st} {body}")
    # Group membership is set explicitly: a create body's `groups` is not honoured everywhere.
    st, tree = h.json("GET", f"/id/admin/realms/{realm}/groups?briefRepresentation=false", headers=auth)
    by_path, todo = {}, list(tree or [])
    while todo:
        g = todo.pop()
        by_path[g.get("path")] = g["id"]
        todo += g.get("subGroups") or []
    for path in groups:
        if path not in by_path:
            raise RuntimeError(f"group {path} of {username}: not in realm {realm}")
        st, body = h.json("PUT", f"/id/admin/realms/{realm}/users/{uid}/groups/{by_path[path]}", headers=auth)
        if st not in (200, 204):
            raise RuntimeError(f"group {path} of {username}: {st} {body}")
    return uid, password


def code_flow(h: Http, realm: str, client: dict, password: str):
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    redirect = client["redirectUris"][0].replace("*", "rehearsal")
    q = urllib.parse.urlencode({"client_id": client["clientId"], "redirect_uri": redirect, "response_type": "code",
                                "scope": "openid", "state": "rh", "code_challenge": challenge,
                                "code_challenge_method": "S256"})
    login_url, st, page = follow(h, f"/id/realms/{realm}/protocol/openid-connect/auth?{q}")
    if st != 200:
        return False, f"auth -> {st} {page[:200]!r}"
    text = page.decode(errors="replace")
    try:
        form = {"csrf_token": _hidden(text, "csrf_token"), "session_code": _hidden(text, "session_code"),
                "username": TESTER, "password": password}
    except RuntimeError as e:
        return False, str(e)
    st, hdrs, body = h.call("POST", login_url, form=form)
    loc = hdrs.get("Location") or hdrs.get("location") or ""
    if "code=" not in loc:
        return False, f"login -> {st} {loc or body[:300]!r}"
    code = urllib.parse.parse_qs(urllib.parse.urlparse(loc).query)["code"][0]
    st, tok = h.json("POST", f"/id/realms/{realm}/protocol/openid-connect/token", form={
        "grant_type": "authorization_code", "client_id": client["clientId"], "code": code,
        "redirect_uri": redirect, "code_verifier": verifier})
    return st == 200 and "id_token" in (tok or {}), f"token -> {st}"


def cmd_smoke(a):
    loopback_only(a.base)
    state, export = Path(a.state), Path(a.export)
    checks = []

    def check(name, ok, detail="", call=None):
        checks.append({"check": name, "result": "pass" if ok else "fail", "detail": detail,
                       **({"call": call} if call and not ok else {})})

    h = Http(a.base)
    # 1. a test person signs in (no person was imported)
    try:
        tok = admin_token(h, state)
        _, pw = make_person(h, tok, "jc", "rehearsal-steward", groups=["/platform-admins"])
        ok, detail = sign_in(h, "rehearsal-steward", pw)
        check("sign-in as a test person", ok, detail, "GET /api/v1/auth/login")
    except Exception as e:  # noqa: BLE001 - every failure is a report row
        check("sign-in as a test person", False, str(e)[:300], "/id/admin/realms/jc/users")
    st, me = h.json("GET", "/api/v1/auth/me")
    check("Portal knows the person", st == 200, f"{st}", "GET /api/v1/auth/me")
    # 2. Portal pages load (the SPA shell and its API reads)
    for path in ["/", "/api/v1/projects", "/api/v1/version"]:
        st, _, body = h.call("GET", path)
        check(f"Portal {path}", st == 200, f"{st} {len(body)} B", f"GET {path}")
    # 3. each imported realm: discovery, a client's secret grant, and the test person's code + PKCE
    #    sign-in through a public client of the old realm (the code is read from the redirect,
    #    never sent to the old host)
    for f in sorted(Path(a.realms).glob("*.json")) if a.realms else []:
        realm = json.loads(f.read_text())
        name = realm["realm"]
        st, disc = h.json("GET", f"/id/realms/{name}/.well-known/openid-configuration")
        check(f"realm {name}: discovery", st == 200, f"{st}", f"GET /id/realms/{name}/.well-known/openid-configuration")
        conf = next((c for c in realm.get("clients", []) if str(c.get("secret", "")).startswith("$(")
                     and c.get("serviceAccountsEnabled")), None)
        if conf:
            var = re.search(r"\$\((?:env:)?([A-Z0-9_]+)\)", conf["secret"]).group(1)
            secret = (state.parent / "secrets" / f"realm-{var}").read_text().strip()
            st, tok = h.json("POST", f"/id/realms/{name}/protocol/openid-connect/token", form={
                "grant_type": "client_credentials", "client_id": conf["clientId"], "client_secret": secret})
            check(f"realm {name}: client {conf['clientId']} gets a token", st == 200 and "access_token" in (tok or {}),
                  f"{st} {'' if st == 200 else str(tok)[:200]}", f"POST /id/realms/{name}/protocol/openid-connect/token")
        publics = [c for c in realm.get("clients", []) if c.get("publicClient")
                   and c.get("standardFlowEnabled", True) and c.get("redirectUris")]
        # The Portal's own sign-in client first; a client asking for consent last.
        publics.sort(key=lambda c: (c["clientId"] != "portal-ui", bool(c.get("consentRequired"))))
        public = publics[0] if publics else None
        if public:
            ok, detail = code_flow(Http(a.base), name, public,
                                   (state.parent / "secrets" / "realm-REHEARSAL_TESTER_PASSWORD").read_text().strip())
            check(f"realm {name}: test person signs in through {public['clientId']}", ok, detail,
                  f"/id/realms/{name}/protocol/openid-connect/auth")
    # 4. every exported repository is in shako with the exported files
    listing = json.loads((export / "gitea" / "repos.json").read_text())
    auth = {"Authorization": f"token {read_secret(state, 'shako-token')}"}
    missing = []
    for r in listing:
        st, _ = h.json("GET", f"/git/api/v1/projects/{r['owner']}/repositories/{r['name']}", headers=auth)
        if st != 200:
            missing.append(f"{r['owner']}/{r['name']}: {st}")
    check(f"shako: {len(listing)} repositories", not missing, "; ".join(missing[:10]),
          "GET /git/api/v1/projects/{owner}/repositories/{repo}")
    # 5. pipelines render and run on ekiben: the Portal's view of its pipelines
    # The reconciler's first sync takes ~40 s over ~3400 manifests: wait for it, up to 3 min.
    for _ in range(36):
        st, projects = h.json("GET", "/api/v1/projects")
        rows = projects if isinstance(projects, list) else (projects or {}).get("items", []) \
            if isinstance(projects, dict) else []
        names = [p.get("name") or p.get("metadata", {}).get("name") for p in rows]
        if names:
            break
        time.sleep(5)
    check("Portal lists the imported projects", bool(names), f"{st} {names[:12]}", "GET /api/v1/projects")
    for p in names:
        st, pipes = h.json("GET", f"/api/v1/projects/{p}/pipelines")
        items = pipes if isinstance(pipes, list) else (pipes or {}).get("items", []) if isinstance(pipes, dict) else []
        check(f"{p}: pipelines listed", st == 200, f"{st} {len(items)}", f"GET /api/v1/projects/{p}/pipelines")
    ek = read_secret(state, "ekiben-token")
    st, streams = h.json("GET", "/streams/streams", headers={"Authorization": f"Bearer {ek}"})
    check("ekiben: pipelines rendered into streams", st == 200 and bool(streams), f"{st} {str(streams)[:200]}",
          "GET /streams/streams")
    # 6. catalogue datasets in dataroom
    st, body = h.json("GET", "/opendata/api/3/action/package_search?rows=1")
    count = (body or {}).get("result", {}).get("count") if isinstance(body, dict) else None
    check("dataroom: catalogue datasets", st == 200 and bool(count), f"{st} count={count}",
          "GET /opendata/api/3/action/package_search")
    # 7. dashboards in daiya
    pw = read_secret(state, "dashboards-password")
    dauth = {"Authorization": "Basic " + base64.b64encode(f"portal:{pw}".encode()).decode()}
    dashdir = Path(a.dashboards) if a.dashboards else export / "grafana" / "dashboards"
    files = sorted(dashdir.glob("*.json"))
    # each project's dashboard sits in its own organization, the platform models in the main one
    orgs = {f.name.split(".", 1)[0] for f in files if _daiya_kind(f.name) == "dashboard"}
    found_uids, missing = set(), []
    for org in [None, *sorted(orgs)]:
        hdrs = dict(dauth)
        if org:
            ost, o = h.json("GET", f"/dashboards/api/orgs/name/{urllib.parse.quote(org)}", headers=dauth)
            if ost != 200 or not isinstance(o, dict):
                missing.append(f"organization {org}: {ost}")
                continue
            hdrs["X-Grafana-Org-Id"] = str(o.get("id"))
        st, found = h.json("GET", "/dashboards/api/search?type=dash-db", headers=hdrs)
        found_uids |= {d.get("uid") for d in found} if st == 200 and isinstance(found, list) else set()
    for f in files:
        kind = _daiya_kind(f.name)
        if kind in ("dashboard", "model"):
            doc = json.loads(f.read_text())
            uid = (doc.get("dashboard", doc) if kind == "dashboard" else doc).get("uid")
            if uid not in found_uids:
                missing.append(f"{f.name} ({uid})")
    want = sum(1 for f in files if _daiya_kind(f.name) in ("dashboard", "model"))
    check(f"daiya: {want} dashboards in their organizations", not missing and want > 0,
          "; ".join(missing[:8]) or f"{want} found", "GET /dashboards/api/search (per organization)")
    if any(_daiya_kind(f.name) == "dashboard" for f in files):
        pd = portal_dashboards(h, dashdir)
        su = pd["summary"]
        check(f"Portal opens {su['projectDashboards']} project dashboards",
              su["openInPortal"] == su["projectDashboards"],
              f"{su['openInPortal']} open; panels {su['panels']}, with data {su['panelsWithData']}",
              "GET /api/v1/projects/{p}/grafana/dashboards/{uid}")
    # 8. every public Endpoint answers a query
    eps = []
    root = export / "gitea" / "files"
    for f in root.rglob("endpoints/*.yaml"):
        text = f.read_text()
        if re.search(r"(?m)^  audience: public", text) and (m := re.search(r"(?m)^  slug: ([a-z2-7]+)", text)):
            eps.append(m.group(1))
    bad = []
    for slug in sorted(set(eps)):
        st, hdrs, body = Http(a.base).call("GET", f"/api/endpoint/{slug}/ngsi-ld/v1/types")
        ctype = hdrs.get("Content-Type") or hdrs.get("content-type") or ""
        if st != 200 or "json" not in ctype:
            bad.append(f"{slug}: {st} {ctype} {body[:80]!r}")
    check(f"{len(set(eps))} public Endpoints answer a query", not bad and bool(eps),
          f"{len(bad)} failed: {'; '.join(bad[:5])}", "GET /api/endpoint/{slug}/ngsi-ld/v1/types")
    # 9. the 24 h rule holds in the broker: no instance older than the window (+ one sweep), read
    # in its database (the broker's reads need a subject on its default policy)
    if a.pg_port and a.antares_password_file:
        try:
            n = old_history("127.0.0.1", a.pg_port, "antares", "antares", a.antares_password_file,
                            cutoff_for(utcnow()) - SWEEP)
            check("broker holds no history older than 24 h", n == 0, f"{n} history rows older than the window",
                  "SELECT count(*) FROM attr_instances, attr_rows WHERE observed_at < now() - 25 h")
        except RuntimeError as e:
            check("broker holds no history older than 24 h", False, f"cannot read: {e}", "psql antares")
    else:
        check("broker holds no history older than 24 h", False, "not checked: no --pg-port/--antares-password-file",
              "psql antares")
    Path(a.report).write_text(json.dumps({"base": a.base, "at": iso(utcnow()), "checks": checks}, indent=1))
    for c in checks:
        print(f"{c['result']:4}  {c['check']}  {c['detail'][:160]}")
    return 0 if all(c["result"] == "pass" for c in checks) else 1



# ---- dashboards in the Portal ---------------------------------------------------------------------

def portal_dashboards(h: Http, dashboards: Path) -> dict:
    """Per project with a dashboard file: whether the Portal lists it, opens it, and what each
    panel answers (GET /api/v1/projects/{p}/grafana/dashboards[/{uid}[/panels/{id}]]), as the
    signed-in person `h` holds. Platform models have no project page and are counted apart."""
    out = {"projects": {}, "platformModels": sorted(f.name for f in dashboards.glob("*.json")
                                                      if _daiya_kind(f.name) == "model")}
    for f in sorted(dashboards.glob("*.dashboard.json")):
        project = f.name.split(".", 1)[0]
        uid = json.loads(f.read_text())["dashboard"]["uid"]
        base = f"/api/v1/projects/{project}/grafana/dashboards"
        st, listed = h.json("GET", base)
        items = listed if isinstance(listed, list) else (listed or {}).get("items", []) if isinstance(listed, dict) else []
        row = {"uid": uid, "list": st, "listed": any((i or {}).get("uid") == uid for i in items if isinstance(i, dict))}
        st, dash = h.json("GET", f"{base}/{urllib.parse.quote(uid)}")
        row["open"] = st
        panels = []
        if st == 200 and isinstance(dash, dict):
            body = dash.get("dashboard", dash)
            flat = []
            for p in body.get("panels", []):
                flat.extend([p, *p.get("panels", [])])
            for p in flat:
                if p.get("type") == "row" or "id" not in p:
                    continue
                pst, ans = h.json("GET", f"{base}/{urllib.parse.quote(uid)}/panels/{p['id']}")
                state = (ans.get("state") or ans.get("status") or ans.get("error") or
                         ("frames" if ans.get("frames") is not None else "")) if isinstance(ans, dict) else ""
                detail = (ans.get("message") or ans.get("detail") or "") if isinstance(ans, dict) else str(ans)[:120]
                panels.append({"id": p["id"], "title": p.get("title"), "status": pst, "state": str(state)[:60],
                               "detail": str(detail)[:160]})
        row["panels"] = panels
        out["projects"][project] = row
    opened = sum(1 for r in out["projects"].values() if r["open"] == 200)
    out["summary"] = {"projectDashboards": len(out["projects"]), "openInPortal": opened,
                      "panels": sum(len(r["panels"]) for r in out["projects"].values()),
                      "panelsWithData": sum(1 for r in out["projects"].values() for p in r["panels"]
                                            if p["status"] == 200 and p["state"] in ("frames", "ok", "data"))}
    return out


def cmd_portal_dashboards(a):
    loopback_only(a.base)
    h, state = Http(a.base), Path(a.state)
    tok = admin_token(h, state)
    _, pw = make_person(h, tok, "jc", "rehearsal-dashboards", groups=["/platform-admins"])
    ok, detail = sign_in(h, "rehearsal-dashboards", pw)
    if not ok:
        raise SystemExit(f"sign-in failed: {detail}")
    out = portal_dashboards(h, Path(a.dashboards))
    Path(a.report).write_text(json.dumps(out, indent=1))
    print(json.dumps(out["summary"]))


# ---- policy after the import ---------------------------------------------------------------------

PROBE_TENANT = "rehearsal-probe"
PROBE = {"id": "urn:ngsi-ld:RehearsalProbe:anonymous", "type": "RehearsalProbe"}


def policy_problems(write_status: int, engine) -> list[str]:
    """What is wrong with a broker that should serve on its default policy: an anonymous write
    must answer 401 and /q/health must name the joinedcontext engine."""
    problems = []
    if write_status != 401:
        problems.append(f"an anonymous write answered {write_status}, not 401")
    if engine != "joinedcontext":
        problems.append(f"the broker's policy engine is {engine!r}, not 'joinedcontext'")
    return problems


def cmd_policy_check(a):
    """After an import under allow-all: the restarted broker refuses an anonymous write."""
    loopback_only(a.base)
    h = Http(a.base)
    st, _ = h.json("POST", "/broker/ngsi-ld/v1/entities", body=PROBE,
                   headers={"NGSILD-Tenant": PROBE_TENANT})
    if 200 <= st < 300:  # the probe got in: take it out again
        h.call("DELETE", f"/broker/ngsi-ld/v1/entities/{urllib.parse.quote(PROBE['id'], safe='')}",
               headers={"NGSILD-Tenant": PROBE_TENANT})
    hst, health = h.json("GET", "/broker/q/health", headers={
        "Authorization": f"Bearer {read_secret(Path(a.state), 'antares-admin-token')}"})
    engine = (health or {}).get("policy", {}).get("engine") if hst == 200 and isinstance(health, dict) else f"unknown ({hst})"
    problems = policy_problems(st, engine)
    print(json.dumps({"anonymousWrite": st, "engine": engine, "problems": problems}))
    return 1 if problems else 0


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("render"); s.add_argument("--deployment", required=True)
    s.add_argument("--env", required=True); s.add_argument("--out", required=True)
    s = sub.add_parser("synth"); s.add_argument("--render", required=True)
    s.add_argument("--env", required=True); s.add_argument("--out", required=True)
    s = sub.add_parser("realms"); s.add_argument("--export", required=True); s.add_argument("--out", required=True)
    s.add_argument("--kaisatsu-bin", help="leave out the mappers this kaisatsu refuses (named in the report)")
    s = sub.add_parser("import"); s.add_argument("--export", required=True); s.add_argument("--base", required=True)
    s.add_argument("--state", required=True); s.add_argument("--report", required=True)
    s.add_argument("--now"); s.add_argument("--only"); s.add_argument("--daiya-bin")
    s.add_argument("--dashboards", help="Grafana provisioning files (rehearsal.py dashboards) instead of the bundle's")
    s = sub.add_parser("dashboards"); s.add_argument("--render", required=True); s.add_argument("--out", required=True)
    s = sub.add_parser("retain"); s.add_argument("--host", default="127.0.0.1"); s.add_argument("--port", required=True)
    s.add_argument("--user", default="antares"); s.add_argument("--db", default="antares")
    s.add_argument("--password-file", required=True); s.add_argument("--now")
    s.add_argument("--every", type=int, default=0, help="repeat every N seconds (0: once)")
    s = sub.add_parser("smoke"); s.add_argument("--base", required=True); s.add_argument("--state", required=True)
    s.add_argument("--export", required=True); s.add_argument("--report", required=True); s.add_argument("--realms")
    s.add_argument("--pg-port"); s.add_argument("--antares-password-file")
    s.add_argument("--dashboards", help="the Grafana files imported into daiya (rehearsal.py dashboards)")
    s = sub.add_parser("portal-dashboards"); s.add_argument("--base", required=True); s.add_argument("--state", required=True)
    s.add_argument("--dashboards", required=True); s.add_argument("--report", required=True)
    s = sub.add_parser("policy-check"); s.add_argument("--base", required=True); s.add_argument("--state", required=True)
    a = p.parse_args(argv)
    return {"portal-dashboards": cmd_portal_dashboards, "policy-check": cmd_policy_check, "dashboards": cmd_dashboards, "render": cmd_render, "synth": cmd_synth, "realms": cmd_realms, "import": cmd_import,
            "retain": cmd_retain, "smoke": cmd_smoke}[a.cmd](a) or 0


if __name__ == "__main__":
    sys.exit(main())
