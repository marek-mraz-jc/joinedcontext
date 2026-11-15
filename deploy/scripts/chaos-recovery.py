#!/usr/bin/env python3
"""The recovery test on dev (T-5117, OPS-11): kill one part at a time under load, time the healing,
count the writes it lost.

    scripts/chaos-recovery.py --out <summary.json> [--targets broker,gateway,portal,postgres,pipeline-runner]
    scripts/chaos-recovery.py --check      # resolves every pod and credential, deletes nothing

Reads (`--read-rate`, 50/s) and writes (`--write-rate`, 5/s) go through the banskabystrica
conformance Endpoint (its own space, F-BNT1-002) on the api host while the test deletes, one target after the other, the
broker pod, the gateway pod, the Portal's reconciling leader, the primary of postgres-cluster and
the pipeline-runner pod (grace period 1 s: a crash, not a drain). Per target it records:

- `readyS`: from the delete until every pod of the workload is Ready again and none is the
  killed one (by uid: CNPG brings a lost instance back under the same name);
- `lastFailureS`: the last failed read or write after the delete; `recoveredS` is the larger of the two;
- the failed reads and writes in that window. A 429 is the edge's budget, counted as throttled.

Every write creates its own entity, so a write the broker took twice answers 409. After the last
target every acknowledged (201) write is read back: `lost` must be 0, `duplicates` must be 0.
Pipelines: the steward's pipeline health of each project is read before and after; a pipeline
that ran fine before and is failing since the test started is `stuck`. The test's entities are
deleted at the end. Exit 0 when every target healed by itself within `--timeout` and nothing was
lost, duplicated or stuck.

The edge allows 600 requests a minute per token and address on an Endpoint route, so the read
load is spread over one client_credentials token per 9 requests a second. Secrets are read from
the cluster and only ever sent to the realm and the Endpoint; none is printed or written.
`just dev-chaos` runs it behind the dev guard. Announce it with tasks/note first: dev answers
errors while a part is down.
"""

from __future__ import annotations

import argparse
import base64
import json
import math
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime
from pathlib import Path

NAMESPACE = "dev"
DOMAIN = "dev.joinedcontext.com"
REALM = "dev"
PG_CLUSTER = "postgres-cluster"
# target -> (how its victim is chosen, the workload it belongs to)
TARGETS = {
    "broker": ("deployment", "context-broker"),
    "gateway": ("deployment", "context-gateway"),
    "portal": ("leader", "portal"),
    "postgres": ("primary", PG_CLUSTER),
    "pipeline-runner": ("deployment", "pipeline-runner"),
}
# The Portal's reconciler lock (joinedcontext-portal src/reconciler/leader.rs RECONCILER_LOCK_KEY):
# a bigint advisory lock shows in pg_locks as classid = high 32 bits, objid = low 32 bits.
LEADER_KEY = 0x6A63_5F72_6563_6F6E
WRITER = "banskabystrica-conformance"
STEWARD_CLIENT = "apisix-gateway"
PROJECTS = ("banskabystrica", "helsinki")
EDGE_PER_TOKEN = 9  # requests a second, under the edge's 600 a minute per token and address
HTTP_TIMEOUT = 10


def kubectl(*args: str, timeout: int = 60) -> str:
    run = subprocess.run(["kubectl", *args], check=False, capture_output=True, text=True, timeout=timeout)
    if run.returncode != 0:
        raise RuntimeError(f"kubectl {' '.join(args[:3])}: {run.stderr.strip()[-300:]}")
    return run.stdout


def secret(name: str, key: str) -> str:
    raw = kubectl("get", "secret", name, "-n", NAMESPACE, "-o", f"jsonpath={{.data.{key}}}").strip()
    if not raw:
        raise RuntimeError(f"secret {NAMESPACE}/{name} has no {key}")
    return base64.b64decode(raw).decode()


def http(method: str, url: str, token: str | None = None, body: dict | None = None,
         form: dict | None = None) -> tuple[int, bytes]:
    """(status, body); status 0 when no answer came (refused, reset, timeout)."""
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    data = None
    if body is not None:
        data, headers["Content-Type"] = json.dumps(body).encode(), "application/json"
    elif form is not None:
        data, headers["Content-Type"] = urllib.parse.urlencode(form).encode(), "application/x-www-form-urlencoded"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT) as answer:
            return answer.status, answer.read()
    except urllib.error.HTTPError as error:
        return error.code, b""
    except (OSError, ValueError):
        return 0, b""


class Token:
    """A token that renews itself a minute before it expires; an old one is kept while the realm is down."""

    def __init__(self, url: str, form: dict):
        self.url, self.form, self.value, self.renew_at = url, form, "", 0.0
        self.lock = threading.Lock()

    def get(self) -> str:
        with self.lock:
            if time.monotonic() >= self.renew_at:
                status, body = http("POST", self.url, form=self.form)
                if status == 200:
                    answer = json.loads(body)
                    self.value = answer["access_token"]
                    self.renew_at = time.monotonic() + max(int(answer.get("expires_in", 300)) - 60, 10)
                elif not self.value:
                    raise RuntimeError(f"no token for {self.form.get('client_id')} ({status})")
                else:
                    self.renew_at = time.monotonic() + 5
            return self.value


# ---- what is measured, kept free of the cluster so tests/test_chaos_recovery.py can feed it ----

def selector(match_labels: dict) -> str:
    return ",".join(f"{k}={v}" for k, v in sorted(match_labels.items()))


def pods(items: list[dict]) -> list[dict]:
    out = []
    for pod in items:
        ready = any(c.get("type") == "Ready" and c.get("status") == "True"
                    for c in pod.get("status", {}).get("conditions", []))
        out.append({"name": pod["metadata"]["name"], "uid": pod["metadata"]["uid"],
                    "ip": pod.get("status", {}).get("podIP", ""), "ready": ready,
                    "deleting": "deletionTimestamp" in pod["metadata"]})
    return sorted(out, key=lambda p: p["name"])


def healed(current: list[dict], killed_uid: str, want: int) -> bool:
    live = [p for p in current if not p["deleting"]]
    return all(p["uid"] != killed_uid for p in current) and sum(p["ready"] for p in live) >= max(want, 1)


def leader_key_parts(key: int = LEADER_KEY) -> tuple[int, int]:
    return key >> 32, key & 0xFFFF_FFFF


def leader_pod(candidates: list[dict], holder_addrs: list[str]) -> dict:
    """The Portal pod holding the reconciler lock: the only one, or the one whose address holds it."""
    if len(candidates) == 1:
        return candidates[0]
    found = [p for p in candidates if p["ip"] and p["ip"] in holder_addrs]
    if len(found) != 1:
        raise RuntimeError(f"cannot tell the Portal leader: lock held from {holder_addrs or 'nowhere'}, "
                           f"pods {[p['ip'] for p in candidates]} (a meshed database hides the client address)")
    return found[0]


def window(events: list[tuple[float, str, int]], start: float, end: float) -> dict:
    """Failed reads and writes in [start, end); a 429 is throttled, not failed."""
    inside = [e for e in events if start <= e[0] < end]
    ok = {"read": (200, 429), "write": (201, 409, 429)}
    failed = [e for e in inside if e[2] not in ok[e[1]]]
    return {
        "reads": sum(e[1] == "read" for e in inside),
        "failedReads": sum(e[1] == "read" for e in failed),
        "writes": sum(e[1] == "write" for e in inside),
        "failedWrites": sum(e[1] == "write" for e in failed),
        "throttled": sum(e[2] == 429 for e in inside),
        "lastFailureS": round(max(e[0] for e in failed) - start, 1) if failed else 0.0,
    }


def writes_verdict(outcomes: dict[str, int], present: dict[str, bool]) -> dict:
    """outcomes: id -> write status; present: id -> found when read back."""
    acked = [i for i, s in outcomes.items() if s == 201]
    unknown = [i for i, s in outcomes.items() if s == 0 or s >= 500]
    return {
        "acknowledged": len(acked),
        "lost": sum(not present.get(i, False) for i in acked),
        "duplicates": sum(s == 409 for s in outcomes.values()),
        "unknownOutcome": len(unknown),
        "landedWithoutAck": sum(present.get(i, False) for i in unknown),
    }


def stuck(before: list[dict], after: list[dict], since: str) -> list[str]:
    """Pipelines that ran fine before the test and fail with an error from after its start."""
    fine = {h["pipeline"] for h in before if h.get("state") in ("ok", "empty", "drift")}
    return sorted(h["pipeline"] for h in after if h["pipeline"] in fine and h.get("state") == "failing"
                  and (h.get("lastErrorAt") or "") >= since)


def passed(summary: dict) -> bool:
    w = summary["writes"]
    return (all(t["healed"] for t in summary["targets"]) and w["lost"] == 0 and w["duplicates"] == 0
            and not summary["pipelines"]["stuck"] and summary["pipelines"]["measured"])


# ---- the run against dev ----

class Load:
    def __init__(self, api: str, slug: str, org: str, run: str, readers: list[Token], writer: Token):
        self.base = f"{api}/api/endpoint/{slug}/ngsi-ld/v1/entities"
        self.org, self.run, self.readers, self.writer = org, run, readers, writer
        self.events: list[tuple[float, str, int]] = []
        self.outcomes: dict[str, int] = {}
        self.lock, self.stop = threading.Lock(), threading.Event()
        self.pool = ThreadPoolExecutor(max_workers=640)  # 64 a second for the 10 s a dead part holds each

    def urn(self, n: int) -> str:
        return f"urn:ngsi-ld:AirQualityObserved:{self.org}:conformance:chaos-{self.run}-{n:06d}"

    def read(self, n: int) -> None:
        token = self.readers[n % len(self.readers)].get()
        status, _ = http("GET", f"{self.base}?type=AirQualityObserved&limit=1", token)
        with self.lock:
            self.events.append((time.monotonic(), "read", status))

    def write(self, n: int) -> None:
        urn = self.urn(n)
        body = {"id": urn, "type": "AirQualityObserved",
                "dateObserved": {"type": "Property", "value": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")}}
        status, _ = http("POST", self.base, self.writer.get(), body)
        with self.lock:
            self.events.append((time.monotonic(), "write", status))
            self.outcomes[urn] = status

    def pace(self, rate: float, job) -> None:
        start, n = time.monotonic(), 0
        while not self.stop.is_set():
            self.pool.submit(job, n)
            n += 1
            time.sleep(max(start + n / rate - time.monotonic(), 0))

    def start(self, read_rate: float, write_rate: float) -> None:
        for rate, job in ((read_rate, self.read), (write_rate, self.write)):
            threading.Thread(target=self.pace, args=(rate, job), daemon=True).start()

    def finish(self) -> None:
        self.stop.set()
        self.pool.shutdown(wait=True)

    def present(self, urns: list[str]) -> dict[str, bool]:
        """Read each entity back; an answer other than 200/404 is asked again, three times."""
        def one(urn: str) -> tuple[str, bool]:
            for _ in range(3):
                status, _ = http("GET", f"{self.base}/{urllib.parse.quote(urn, safe='')}", self.writer.get())
                if status in (200, 404):
                    return urn, status == 200
                time.sleep(2)
            return urn, False
        with ThreadPoolExecutor(max_workers=8) as pool:
            return dict(pool.map(one, urns))

    def clean(self, urns: list[str]) -> int:
        def one(urn: str) -> bool:
            status, _ = http("DELETE", f"{self.base}/{urllib.parse.quote(urn, safe='')}", self.writer.get())
            return status == 204
        with ThreadPoolExecutor(max_workers=8) as pool:
            return sum(pool.map(one, urns))


def workload_pods(kind: str, name: str) -> tuple[list[dict], int]:
    if kind == "primary":
        cluster = json.loads(kubectl("get", "clusters.postgresql.cnpg.io", name, "-n", NAMESPACE, "-o", "json"))
        items = json.loads(kubectl("get", "pods", "-n", NAMESPACE, "-l", f"cnpg.io/cluster={name}", "-o", "json"))
        return pods([p for p in items["items"] if "cnpg.io/jobRole" not in p["metadata"].get("labels", {})]), \
            int(cluster["spec"].get("instances", 1))
    deploy = json.loads(kubectl("get", "deployment", name, "-n", NAMESPACE, "-o", "json"))
    items = json.loads(kubectl("get", "pods", "-n", NAMESPACE, "-l",
                               selector(deploy["spec"]["selector"]["matchLabels"]), "-o", "json"))
    return pods(items["items"]), int(deploy["spec"].get("replicas", 1))


def primary_pod() -> str:
    name = kubectl("get", "pods", "-n", NAMESPACE, "-l", f"cnpg.io/cluster={PG_CLUSTER},cnpg.io/instanceRole=primary",
                   "-o", "jsonpath={.items[0].metadata.name}").strip()
    if not name:
        raise RuntimeError(f"no primary pod of {NAMESPACE}/{PG_CLUSTER}")
    return name


def victim(target: str) -> dict:
    kind, name = TARGETS[target]
    current, _ = workload_pods(kind, name)
    live = [p for p in current if p["ready"] and not p["deleting"]]
    if not live:
        raise RuntimeError(f"{target}: no Ready pod to kill; dev is not healthy, fix that first")
    if kind == "primary":
        wanted = primary_pod()
        found = next((p for p in live if p["name"] == wanted), None)
        if found is None:
            raise RuntimeError(f"postgres: the primary {wanted} is not Ready; dev is not healthy, fix that first")
        return found
    if kind == "leader":
        hi, lo = leader_key_parts()
        sql = (f"select coalesce(host(a.client_addr), '') from pg_locks l join pg_stat_activity a using (pid) "
               f"where l.locktype = 'advisory' and l.granted and l.classid = {hi} and l.objid = {lo}")
        addrs = [] if len(live) == 1 else kubectl(
            "exec", "-n", NAMESPACE, primary_pod(), "-c", "postgres", "--", "psql", "-At", "-c", sql).split()
        return leader_pod(live, addrs)
    return live[0]


def pipeline_health(portal: str, steward: Token | None) -> list[dict] | None:
    if steward is None:
        return None
    out = []
    for project in PROJECTS:
        status, body = http("GET", f"{portal}/api/v1/projects/{project}/pipeline-health", steward.get())
        if status != 200:
            return None
        out += json.loads(body)["items"]
    return out


def kill_and_watch(target: str, load: Load, timeout: int, settle: int) -> dict:
    kind, name = TARGETS[target]
    pod = victim(target)
    _, want = workload_pods(kind, name)
    killed_at, wall = time.monotonic(), datetime.now(UTC).isoformat(timespec="seconds")
    kubectl("delete", "pod", pod["name"], "-n", NAMESPACE, "--grace-period=1", "--wait=false")
    print(f"{wall} killed {target} pod {pod['name']}", flush=True)
    ready_s = None
    while time.monotonic() - killed_at < timeout:
        time.sleep(2)
        try:
            current, want = workload_pods(kind, name)
        except RuntimeError:
            continue
        if healed(current, pod["uid"], want):
            ready_s = round(time.monotonic() - killed_at, 1)
            break
    time.sleep(settle)
    with load.lock:
        measured = window(list(load.events), killed_at, time.monotonic())
    result = {"target": target, "pod": pod["name"], "killedAt": wall, "readyS": ready_s,
              "healed": ready_s is not None, **measured,
              "recoveredS": max(ready_s or float(timeout), measured["lastFailureS"])}
    print(f"  {target}: ready {ready_s} s, last failure {measured['lastFailureS']} s, "
          f"failed reads {measured['failedReads']}/{measured['reads']}, "
          f"failed writes {measured['failedWrites']}/{measured['writes']}", flush=True)
    return result


def table(summary: dict) -> str:
    rows = ["| target | pod Ready (s) | last failure (s) | failed reads | failed writes | throttled |",
            "|---|---|---|---|---|---|"]
    rows += [f"| {t['target']} | {t['readyS'] if t['healed'] else 'never'} | {t['lastFailureS']} | "
             f"{t['failedReads']}/{t['reads']} | {t['failedWrites']}/{t['writes']} | {t['throttled']} |"
             for t in summary["targets"]]
    w, p = summary["writes"], summary["pipelines"]
    rows.append(f"\nwrites: {w['acknowledged']} acknowledged, {w['lost']} lost, {w['duplicates']} duplicated, "
                f"{w['unknownOutcome']} without an answer ({w['landedWithoutAck']} of them stored); "
                f"pipelines: {'not measured' if not p['measured'] else (', '.join(p['stuck']) or 'none') + ' stuck'}")
    return "\n".join(rows)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--out", type=Path, default=Path("deployment/chaos-recovery.json"))
    parser.add_argument("--targets", default=",".join(TARGETS))
    parser.add_argument("--read-rate", type=float, default=50.0)
    parser.add_argument("--write-rate", type=float, default=5.0)
    parser.add_argument("--timeout", type=int, default=600, help="seconds a target may take to be Ready again")
    parser.add_argument("--settle", type=int, default=60, help="seconds of load after Ready, before the next target")
    parser.add_argument("--check", action="store_true", help="resolve pods and credentials, delete nothing")
    args = parser.parse_args()
    targets = [t for t in args.targets.split(",") if t]
    unknown = [t for t in targets if t not in TARGETS]
    if unknown or not targets:
        print(f"unknown target {unknown}; choose from {', '.join(TARGETS)}", file=sys.stderr)
        return 2
    if not 0 < args.read_rate <= 200 or not 0 < args.write_rate <= 20:
        print("--read-rate is 1..200 and --write-rate 1..20 a second", file=sys.stderr)
        return 2

    api, portal, idm = f"https://api.{DOMAIN}", f"https://portal.{DOMAIN}", f"https://idm.{DOMAIN}"
    token_url = f"{idm}/realms/{REALM}/protocol/openid-connect/token"
    org = kubectl("get", "deploy", "context-gateway", "-n", NAMESPACE, "-o",
                  "jsonpath={.spec.template.spec.containers[0].env[?(@.name==\"JC_GATEWAY_ORG_DOMAIN\")].value}").strip()
    seed = next((n.split("/", 1)[1] for n in kubectl("get", "configmaps", "-n", NAMESPACE, "-o", "name").split()
                 if n.split("/", 1)[1].startswith("gitea-seed-") and n.endswith("banskabystrica")), "")
    manifest = kubectl("get", "configmap", seed, "-n", NAMESPACE, "-o",
                       r"jsonpath={.data.projects__banskabystrica__spaces__conformance__endpoints__conformance\.yaml}") if seed else ""
    slug = next((line.split(":", 1)[1].strip() for line in manifest.splitlines() if line.strip().startswith("slug:")), "")
    if not org or not slug:
        raise RuntimeError("the gateway's org domain or the conformance Endpoint's slug is not on dev")
    writer_form = {"grant_type": "client_credentials", "client_id": WRITER,
                   "client_secret": secret(f"keycloak-client-{WRITER}", "client-secret")}
    readers = [Token(token_url, dict(writer_form)) for _ in range(math.ceil(args.read_rate / EDGE_PER_TOKEN))]
    writer = Token(token_url, dict(writer_form))
    try:
        steward = Token(token_url, {"grant_type": "password", "client_id": STEWARD_CLIENT,
                                    "client_secret": secret(f"keycloak-client-{STEWARD_CLIENT}", "client-secret"),
                                    "username": f"demo.steward@{org}",
                                    "password": secret("keycloak-user-demo-steward", "password")})
        steward.get()
    except RuntimeError as error:
        print(f"pipelines not measured: {error}", file=sys.stderr)
        steward = None
    for t in [writer, *readers]:
        t.get()
    victims = {t: victim(t)["name"] for t in targets}
    print(f"dev {org}, endpoint {slug}, {len(readers)} read tokens; would kill: {victims}", flush=True)
    if args.check:
        return 0

    run = datetime.now(UTC).strftime("%Y%m%d%H%M%S")
    started = datetime.now(UTC).isoformat(timespec="seconds")
    before = pipeline_health(portal, steward)
    load = Load(api, slug, org, run, readers, writer)
    load.start(args.read_rate, args.write_rate)
    results = []
    try:
        time.sleep(args.settle)  # a baseline under load before the first kill
        for target in targets:
            try:
                results.append(kill_and_watch(target, load, args.timeout, args.settle))
            except RuntimeError as error:
                print(f"  {target}: {error}", file=sys.stderr)
                results.append({"target": target, "healed": False, "error": str(error), "readyS": None,
                                "lastFailureS": 0.0, "reads": 0, "failedReads": 0, "writes": 0,
                                "failedWrites": 0, "throttled": 0, "recoveredS": None})
    finally:
        load.finish()
        written = list(load.outcomes)
        present = load.present(written)
        removed = load.clean([u for u, here in present.items() if here])
    after = pipeline_health(portal, steward)
    summary = {
        "run": run, "started": started, "readRate": args.read_rate, "writeRate": args.write_rate,
        "targets": results, "writes": writes_verdict(dict(load.outcomes), present), "cleanedUp": removed,
        "pipelines": {"measured": before is not None and after is not None,
                      "stuck": stuck(before or [], after or [], started)},
    }
    summary["passed"] = passed(summary) and len(results) == len(targets)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(summary, indent=2) + "\n")
    print(table(summary))
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
