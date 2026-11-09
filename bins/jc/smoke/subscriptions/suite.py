"""Smoke suite `subscriptions`: NGSI-LD subscriptions over the Endpoint WebSockets and the edge
MQTT brokers, against production's transports as the export declares them.

   bins/jc/smoke/run subscriptions --base-url <url> --export <bundle> --out <dir> --state <state>
       [--per-class 1] [--wait 15] [--silence 5]
       [--mqtt-broker <project>/<broker>=mqtt://127.0.0.1:<port>]... [--mqtt-token-dir <dir>]

1. transport: every exported Endpoint's `/api/endpoint/{slug}/ws` without an upgrade answers as
   its transports say: the guide (200) when it lists `websocket` and is public, 401 when it is
   closed, 404 when it does not list it; an unknown slug 404s (the control). Production is read
   from the export alone: this suite opens no subscription on prod or dev.
2. delivery, per class (project x audience) of public Endpoints serving `websocket`, on a type its
   public grant opens without an id pattern or a q: subscriber A gets the write of a probe entity
   (through the broker, as the import writes) within --wait seconds with the TS 104 243 envelope
   (x-seq 1, x-subscription the created id); B on the same Endpoint gets it too (one fan-out hub
   per Endpoint, ADR-N-049); C, whose q excludes the probe, gets nothing; after A's x-unsubscribe
   a second write reaches B and not A (ws_delivery.rs, ws_hub.rs).
3. outside the grant: D on another tenant's Endpoint, subscribed to the same type, gets nothing;
   E on the same Endpoint subscribed to a type no public grant opens gets nothing or a refusal;
   an anonymous upgrade of a closed Endpoint is 401; the 11th anonymous socket of one caller 429s
   (EP-107). A silence passes only when A's delivery in the same window passed.
4. MQTT, per exported MqttBroker: CONNECT with a ServiceAccount token (checked by the gateway's
   /internal/mqtt/auth), QoS 1 publish on each declared topic, publish outside the role and a
   CONNECT with a bad token refused, and each declared subscribe topic. jc runs no MQTT broker:
   the brokers stay a component on dev beside jc (GOAL-jc-dev "Scope of jc on dev", CUTOVER.md
   section 1), so without --mqtt-broker every check is a named skip "kept component". After the
   move: kubectl port-forward each broker's listener to loopback and give --mqtt-broker and
   --mqtt-token-dir (<dir>/<project>/<serviceAccount>: an access token of audience the broker's
   release, never a secret in argv). The gateway's /internal/mqtt/auth must never answer on the
   public base (checked here on any jc).

--state needs `antares-admin-token` (the probe writes); without it parts 2 and 3 are skipped."""

from __future__ import annotations

import argparse
import json
import secrets
import sys
import time
import urllib.parse
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
SMOKE = HERE.parent
for p in (HERE, SMOKE, SMOKE / "endpoints"):
    sys.path.insert(0, str(p))

import export as ex
import mqttclient
import wsclient
from common import Http, Results, as_json, jwt_claims, loopback_only, short

DEFAULT_CONTEXT = "https://uri.etsi.org/ngsi-ld/default-context/"
MARK = "smokeSubsMark"
OUTSIDE_TYPE = DEFAULT_CONTEXT + "SmokeSubsOutside"
PER_CALLER = 10  # the gateway's WebSocket connections per caller (handlers/ws.rs PER_CALLER)
KEPT = ("kept component: the MqttBroker {b} stays a separate component on dev beside jc, stamped by jc's "
        "Portal reconciler through Kubernetes (GOAL-jc-dev 'Scope of jc on dev', CUTOVER.md section 1); "
        "the docker rehearsal has no Kubernetes. After the move: --mqtt-broker {b}=mqtt://127.0.0.1:<port-forward>")


def ws_path(slug: str) -> str:
    return f"/api/endpoint/{urllib.parse.quote(slug, safe='')}/ws"


def is_op(name: str):
    return lambda m: wsclient.op(m) == name


def carries(eid: str):
    """A notification whose data holds entity `eid`."""
    def pred(m):
        if wsclient.op(m) != "x-notification":
            return False
        data = (m.get("body") or {}).get("data") or []
        return any(isinstance(x, dict) and x.get("id") == eid for x in data)
    return pred


# ------------------------------------------------------------------------------------ export

def open_types(e: ex.Endpoint, tenant_types: dict) -> dict:
    """{short type: slot} the public grants open whole: no id pattern and no q/scopeQ/validity, so
    a probe of the type is inside the grant whatever its id."""
    grants = e.public_grants()
    if not grants or any(g.narrowed for g in grants):
        return {}
    sels = e.selectors(grants)
    out = {}
    for st, slot in tenant_types.items():
        if sels is None or any(s.type in (st, "*") and s.pattern is None for s in sels):
            out[st] = slot
    return out


def grants_every_type(e: ex.Endpoint) -> bool:
    return e.selectors(e.public_grants()) is None


def plan(endpoints, index, per_class: int):
    """(chosen [(class, endpoint, short type, iri)], skipped [(endpoint, why)]) for part 2."""
    by_class = defaultdict(list)
    for e in endpoints:
        if "websocket" in e.representations and not e.closed:
            by_class[(e.project, e.audience)].append(e)
    chosen, skipped = [], []
    for k in sorted(by_class):
        n = 0
        for e in sorted(by_class[k], key=lambda x: x.slug):
            types = open_types(e, (index.get("tenants") or {}).get(e.tenant, {}))
            if not types:
                skipped.append((e, "no public grant opens a whole type with rows in the export"))
                continue
            if n >= per_class:
                continue
            st = max(types, key=lambda t: (not types[t].get("expires"), types[t]["count"]))
            chosen.append((f"{k[0]}/{k[1]}", e, st, types[st]["iri"]))
            n += 1
    return chosen, skipped


def load_brokers(files: Path) -> list[dict]:
    out = []
    for f in sorted(files.glob("*/*/mqttbrokers/*.yaml")):
        for d in ex._load_yaml(f):
            if d.get("kind") == "MqttBroker":
                meta = d.get("metadata") or {}
                out.append({"project": meta.get("namespace") or f.parts[-3], "name": meta["name"],
                            "roles": (d.get("spec") or {}).get("roles") or []})
    return out


def concrete(topic: str, client_id: str) -> str:
    """A publishable topic of a filter: ${clientid} the client's id, a wildcard level `smoke`."""
    levels = topic.replace("${clientid}", client_id).split("/")
    return "/".join("smoke" if lv in ("+", "#") else lv for lv in levels)


# ------------------------------------------------------------------------------------ suite

class Suite:
    def __init__(self, a, endpoints, index, brokers):
        self.a, self.endpoints, self.index, self.brokers = a, endpoints, index, brokers
        self.http = Http(a.base_url, timeout=a.timeout)
        self.r = Results("subscriptions", a.base_url)
        self.admin = None
        if a.state and (Path(a.state) / "antares-admin-token").exists():
            self.admin = (Path(a.state) / "antares-admin-token").read_text().strip() or None
        self.probes: list[tuple[str, str]] = []

    # ------------------------------------------------------------ 1. transports

    def transports(self):
        st, _, body = self.http.call("GET", ws_path("smoke-subs-unknown-" + secrets.token_hex(4)))
        self.r.check("control.unknown-slug", "/api/endpoint/<unknown>/ws", "GET (no upgrade)", "404",
                     f"{st} {short(body, 80)}", st == 404)
        for e in self.endpoints:
            st, _, body = self.http.call("GET", ws_path(e.slug))
            listed = "websocket" in e.representations
            if not listed:
                want, ok = "404 (websocket not listed)", st == 404 or (e.closed and st == 401)
            elif e.closed:
                want, ok = "401 (closed, anonymous)", st == 401
            else:
                guide = as_json(body)
                want, ok = "200 guide", st == 200 and isinstance(guide, dict)
            self.r.check(f"transport.{e.slug}", f"{e.project}/{e.name}", f"GET {ws_path(e.slug)}", want,
                         f"{st} {short(body, 100)}", ok)

    # ------------------------------------------------------------ probes

    def write(self, tenant: str, eid: str, iri: str, mark: int) -> tuple[int, bytes]:
        body = {"id": eid, "type": iri, DEFAULT_CONTEXT + MARK: {"type": "Property", "value": mark}}
        st, _, raw = self.http.call("POST", "/broker/ngsi-ld/v1/entityOperations/upsert?options=update",
                                    token=self.admin, headers={"NGSILD-Tenant": tenant}, body=[body])
        if st in (200, 201, 204):
            self.probes.append((tenant, eid))
        return st, raw

    def cleanup(self):
        for tenant, eid in set(self.probes):
            self.http.call("DELETE", f"/broker/ngsi-ld/v1/entities/{urllib.parse.quote(eid, safe='')}",
                           token=self.admin, headers={"NGSILD-Tenant": tenant})

    def socket(self, e: ex.Endpoint, token: str | None = None):
        """(socket, None) once welcomed, or (None, why)."""
        try:
            s = wsclient.Ws(self.a.base_url, ws_path(e.slug), token=token, timeout=self.a.timeout)
        except wsclient.Refused as r:
            return None, f"upgrade {r.status} {short(r.body, 100)}"
        except (OSError, wsclient.Closed) as x:
            return None, f"no socket: {x}"
        try:
            w = s.until(is_op("x-welcome"), self.a.wait)
        except (OSError, wsclient.Closed) as x:
            s.close()
            return None, f"no welcome: {x}"
        if not w:
            s.close()
            return None, "no x-welcome within the wait"
        return s, None

    def subscribe(self, s, body: dict, xid: str):
        """(status, created id or '', message) of an x-subscribe."""
        try:
            s.send({"metadata": {"x-op": "x-subscribe", "x-id": xid}, "body": body})
            m = s.until(lambda m: wsclient.op(m) == "x-result" and (m.get("metadata") or {}).get("x-id") == xid,
                        self.a.wait)
        except (OSError, wsclient.Closed) as x:
            return 0, "", str(x)
        if not m:
            return 0, "", "no x-result within the wait"
        meta = m.get("metadata") or {}
        return int(meta.get("x-status") or 0), str((m.get("body") or {}).get("id") or ""), json.dumps(m)[:200]

    @staticmethod
    def silent(s, pred, wait: float):
        """'' when nothing `pred` names came within `wait`, else what came."""
        if s is None:
            return ""
        try:
            m = s.until(pred, wait)
        except (OSError, wsclient.Closed):
            return ""
        return json.dumps(m)[:200] if m else ""

    # ------------------------------------------------------------ 2 + 3. one class

    def deliver(self, cls: str, e: ex.Endpoint, st: str, iri: str, other: ex.Endpoint | None):
        t, w = f"{e.project}/{e.name}", self.a.wait
        tag = secrets.token_hex(4)
        probe = f"urn:ngsi-ld:{st}:smoke-subs-{tag}"
        sub = {"type": "Subscription", "entities": [{"type": iri}]}
        socks = []
        try:
            a, why = self.socket(e)
            if not a:
                self.r.check(f"deliver.{e.slug}", t, f"WS {ws_path(e.slug)}", "upgrade 101 and x-welcome", why, False,
                             f"class {cls}, type {st}")
                return
            socks.append(a)
            st_a, id_a, msg_a = self.subscribe(a, sub, "a")
            b, _why_b = self.socket(e)
            socks += [b] if b else []
            st_b = self.subscribe(b, sub, "b")[0] if b else 0
            c, _ = self.socket(e)
            socks += [c] if c else []
            st_c = self.subscribe(c, {**sub, "q": f"{MARK}==99"}, "c")[0] if c else 0
            d = None
            if other:
                d, _ = self.socket(other)
                socks += [d] if d else []
                if d:
                    self.subscribe(d, sub, "d")
            ee = None
            if not grants_every_type(e):
                ee, _ = self.socket(e)
                socks += [ee] if ee else []
                st_e = self.subscribe(ee, {"type": "Subscription", "entities": [{"type": OUTSIDE_TYPE}]}, "e")[0] \
                    if ee else 0
            if st_a != 201:
                self.r.check(f"deliver.{e.slug}", t, "x-subscribe", "x-status 201", f"{st_a} {msg_a}", False,
                             f"class {cls}, type {st}")
                return
            time.sleep(self.a.settle)  # the hub's broker subscription is made after the 201
            wst, wraw = self.write(e.tenant, probe, iri, 1)
            outside = f"urn:ngsi-ld:SmokeSubsOutside:smoke-subs-{tag}"
            if ee:
                self.write(e.tenant, outside, OUTSIDE_TYPE, 1)
            if wst not in (200, 201, 204):
                self.r.check(f"deliver.{e.slug}", t, "POST /broker/.../entityOperations/upsert", "the probe is written",
                             f"{wst} {short(wraw)}", False, "the probe write failed: nothing to deliver")
                return
            started = time.monotonic()
            n = a.until(carries(probe), w)
            took = time.monotonic() - started
            got = bool(n)
            self.r.check(f"deliver.{e.slug}", t, f"WS {ws_path(e.slug)} x-subscribe {st}; write {probe}",
                         f"x-notification within {w:g} s", f"after {took:.1f} s" if got else "nothing", got,
                         f"class {cls}, tenant {e.tenant}")
            if got:
                meta = n.get("metadata") or {}
                ok = meta.get("x-seq") == 1 and meta.get("x-subscription") in (id_a, None) and \
                    (n.get("body") or {}).get("subscriptionId") in (id_a, None)
                self.r.check(f"deliver.{e.slug}.envelope", t, "x-notification metadata",
                             f"x-seq 1, x-subscription {id_a}", json.dumps(meta)[:160], ok)
            proven = "" if got else "unproven: A got no delivery in the same window"
            nb = b.until(carries(probe), w) if b and st_b == 201 else None
            self.r.check(f"hub.{e.slug}.second-subscriber", t, "a second socket on the Endpoint, same subscription",
                         "the same notification", f"subscribe {st_b}; {'notified' if nb else 'nothing'}", bool(nb))
            if c and st_c == 201:
                came = self.silent(c, carries(probe), self.a.silence)
                self.r.check(f"hub.{e.slug}.filter", t, f"x-subscribe q {MARK}==99", "nothing (q excludes the probe)",
                             came or "nothing", got and not came, proven)
            else:
                self.r.check(f"hub.{e.slug}.filter", t, f"x-subscribe q {MARK}==99", "x-status 201", str(st_c), False)
            if other:
                came = self.silent(d, carries(probe), 0.5)
                self.r.check(f"outside.{e.slug}.other-endpoint", t,
                             f"WS {ws_path(other.slug)} ({other.tenant}) x-subscribe {st}",
                             "nothing of another tenant's write", came or ("nothing" if d else "no socket"),
                             bool(d) and got and not came, proven)
            if ee:
                came = self.silent(ee, carries(outside), 0.5) if st_e == 201 else ""
                self.r.check(f"outside.{e.slug}.ungranted-type", t, "x-subscribe SmokeSubsOutside",
                             "nothing (or a refusal)", came or f"subscribe {st_e}, nothing", got and not came, proven)
            else:
                self.r.skip(f"outside.{e.slug}.ungranted-type", t, "x-subscribe SmokeSubsOutside",
                            "nothing (or a refusal)", "the public grant opens every type of the tenant")
            # A leaves; the hub keeps serving B.
            try:
                a.send({"metadata": {"x-op": "x-unsubscribe", "x-id": "u", "x-subscription": id_a}})
                un = a.until(lambda m: wsclient.op(m) == "x-result" and (m.get("metadata") or {}).get("x-id") == "u", w)
            except (OSError, wsclient.Closed):
                un = None
            self.write(e.tenant, probe, iri, 2)
            nb2 = b.until(carries(probe), w) if nb else None
            came = self.silent(a, carries(probe), self.a.silence)
            un_st = (un or {}).get("metadata", {}).get("x-status")
            self.r.check(f"hub.{e.slug}.unsubscribe", t, "x-unsubscribe A, then a second write",
                         "A: 204 and nothing; B: notified", f"A {un_st}, {came or 'nothing'}; B {'notified' if nb2 else 'nothing'}",
                         un_st == 204 and not came and bool(nb2))
        finally:
            for s in socks:
                s.close()

    def refusals(self):
        closed = [e for e in self.endpoints if e.closed and "websocket" in e.representations]
        for e in closed:
            try:
                s = wsclient.Ws(self.a.base_url, ws_path(e.slug), timeout=self.a.timeout)
                s.close()
                actual, ok = "101", False
            except wsclient.Refused as r:
                actual, ok = f"{r.status} {short(r.body, 80)}", r.status == 401
            except (OSError, wsclient.Closed) as x:
                actual, ok = f"no answer: {x}", False
            self.r.check(f"refused.{e.slug}.anonymous", f"{e.project}/{e.name}", f"WS {ws_path(e.slug)} (no token)",
                         "401", actual, ok, f"audience {e.audience}")
        public = [e for e in self.endpoints if not e.closed and "websocket" in e.representations]
        if not public:
            return
        e = min(public, key=lambda x: x.slug)
        held, last = [], ""
        time.sleep(2)  # the sockets of part 2 leave the caller's count once their sessions end
        try:
            for _ in range(PER_CALLER):
                s, why = self.socket(e)
                if not s:
                    last = why
                    break
                held.append(s)
            if len(held) == PER_CALLER:
                try:
                    wsclient.Ws(self.a.base_url, ws_path(e.slug), timeout=self.a.timeout).close()
                    last = "101"
                except wsclient.Refused as r:
                    last = f"{r.status}"
            self.r.check("refused.per-caller-cap", f"{e.project}/{e.name}",
                         f"{PER_CALLER + 1} anonymous sockets of one caller", f"{PER_CALLER} open, the next 429",
                         f"{len(held)} open, next {last}", len(held) == PER_CALLER and last == "429")
        finally:
            for s in held:
                s.close()

    def delivery(self):
        if not self.admin:
            for e in self.endpoints:
                if "websocket" in e.representations and not e.closed:
                    self.r.skip(f"deliver.{e.slug}", f"{e.project}/{e.name}", "WS + probe write",
                                "x-notification", "no --state with antares-admin-token: the probe cannot be written")
                    break
            return
        chosen, skipped = plan(self.endpoints, self.index, self.a.per_class)
        for e, why in skipped[:50]:
            self.r.skip(f"deliver.{e.slug}", f"{e.project}/{e.name}", "WS + probe write", "x-notification", why)
        for i, (cls, e, st, iri) in enumerate(chosen):
            other = next((x[1] for x in chosen[i + 1:] + chosen[:i] if x[1].tenant != e.tenant), None)
            self.deliver(cls, e, st, iri, other)

    # ------------------------------------------------------------ 4. MQTT

    def mqtt(self):
        st, _, body = self.http.call("POST", "/internal/mqtt/auth",
                                     body={"clientid": "smoke", "username": "smoke", "password": "x", "ipaddr": "127.0.0.1"})
        self.r.check("mqtt.auth-not-public", "gateway", "POST /internal/mqtt/auth on the public base",
                     "404 (internal listener only)", f"{st} {short(body, 80)}", st == 404)
        given = dict(x.split("=", 1) for x in self.a.mqtt_broker)
        for b in self.brokers:
            name = f"{b['project']}/{b['name']}"
            url = given.get(name)
            rows = [("connect", "CONNECT with a ServiceAccount token", "CONNACK 0"),
                    ("refused-token", "CONNECT with a token that does not verify", "CONNACK refused")]
            for role in b["roles"]:
                rows += [(f"{role['name']}.publish.{i}", f"PUBLISH QoS 1 {t}", "PUBACK")
                         for i, t in enumerate(role.get("publish") or [])]
                rows += [(f"{role['name']}.subscribe.{i}", f"SUBSCRIBE {t}", "SUBACK granted")
                         for i, t in enumerate(role.get("subscribe") or [])]
                rows.append((f"{role['name']}.refused-topic", "PUBLISH outside the role", "refused"))
            if not url:
                for rid, call, want in rows:
                    self.r.skip(f"mqtt.{name}.{rid}", name, call, want, KEPT.format(b=name))
                continue
            loopback_only(url)
            try:
                self.broker(b, name, url)
            except OSError as x:
                self.r.check(f"mqtt.{name}.connect", name, f"TCP {url}", "the broker's listener answers", str(x), False)

    def token_of(self, project: str, account: str) -> str | None:
        f = Path(self.a.mqtt_token_dir or "/nonexistent") / project / account
        return f.read_text().strip() if f.is_file() else None

    def broker(self, b, name, url):
        w = self.a.wait
        bad = mqttclient.Mqtt(url)
        rc = bad.connect("smoke-subs-bad", "smoke-subs-bad", "not-a-token", w)
        bad.close()
        self.r.check(f"mqtt.{name}.refused-token", name, "CONNECT with a token that does not verify",
                     "CONNACK refused", f"rc {rc}", rc != 0)
        connected = False
        for role in b["roles"]:
            account = next(iter(role.get("serviceAccounts") or []), None)
            token = self.token_of(b["project"], account) if account else None
            rid = role["name"]
            if not token:
                why = f"no token file {b['project']}/{account} in --mqtt-token-dir" if account else "the role names no ServiceAccount"
                for i, t in enumerate(role.get("publish") or []):
                    self.r.skip(f"mqtt.{name}.{rid}.publish.{i}", name, f"PUBLISH {t}", "PUBACK", why)
                for i, t in enumerate(role.get("subscribe") or []):
                    self.r.skip(f"mqtt.{name}.{rid}.subscribe.{i}", name, f"SUBSCRIBE {t}", "SUBACK", why)
                self.r.skip(f"mqtt.{name}.{rid}.refused-topic", name, "PUBLISH outside the role", "refused", why)
                continue
            client = jwt_claims(token).get("azp") or account
            m = mqttclient.Mqtt(url)
            rc = m.connect(client, client, token, w)
            if not connected:
                self.r.check(f"mqtt.{name}.connect", name, f"CONNECT {client}", "CONNACK 0", f"rc {rc}", rc == 0)
                connected = True
            if rc != 0:
                m.close()
                continue
            for i, t in enumerate(role.get("subscribe") or []):
                code = m.subscribe(t, w)
                self.r.check(f"mqtt.{name}.{rid}.subscribe.{i}", name, f"SUBSCRIBE {t}", "SUBACK 0..2",
                             f"{code}", 0 <= code <= 2)
            for i, t in enumerate(role.get("publish") or []):
                topic = concrete(t, client)
                ok = m.publish(topic, b'{"smoke": "subscriptions"}', w)
                self.r.check(f"mqtt.{name}.{rid}.publish.{i}", name, f"PUBLISH QoS 1 {topic}", "PUBACK",
                             "acknowledged" if ok else "refused", ok)
            refused = not m.publish(f"smoke-subs-outside/{client}", b"{}", w)
            self.r.check(f"mqtt.{name}.{rid}.refused-topic", name, f"PUBLISH smoke-subs-outside/{client}",
                         "refused (no PUBACK, disconnected)", "refused" if refused else "acknowledged", refused)
            m.close()
        if not connected:
            self.r.skip(f"mqtt.{name}.connect", name, "CONNECT with a ServiceAccount token", "CONNACK 0",
                        "no role's ServiceAccount token in --mqtt-token-dir")

    def run(self):
        try:
            self.transports()
            self.delivery()
            self.refusals()
            self.mqtt()
        finally:
            if self.admin:
                self.cleanup()


def main(argv=None):
    p = argparse.ArgumentParser(prog="run subscriptions", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--base-url", required=True)
    p.add_argument("--export", required=True, help="the export bundle the rehearsal was built from")
    p.add_argument("--out", required=True)
    p.add_argument("--state", help="the rehearsal's state dir: antares-admin-token for the probe writes")
    p.add_argument("--per-class", type=int, default=1, help="Endpoints per project x audience for delivery")
    p.add_argument("--wait", type=float, default=15, help="seconds a delivery may take (N)")
    p.add_argument("--silence", type=float, default=5, help="seconds a socket must stay silent")
    p.add_argument("--settle", type=float, default=1, help="seconds between the subscribe and the write")
    p.add_argument("--timeout", type=float, default=30)
    p.add_argument("--mqtt-broker", action="append", default=[], metavar="PROJECT/BROKER=URL",
                   help="a broker's listener on loopback (a port-forward after the move)")
    p.add_argument("--mqtt-token-dir", help="<dir>/<project>/<serviceAccount>: an access token per file")
    p.add_argument("--cache", default=str(Path.home() / ".cache" / "jc-smoke"))
    p.add_argument("--admin-token-file", help="unused (accepted for the common runner)")
    p.add_argument("--person-token-file", help="unused (accepted for the common runner)")
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    export = Path(a.export)
    files = export / "gitea" / "files"
    endpoints = ex.load_endpoints(files)
    index = ex.load_index(export, endpoints, Path(a.cache)) if a.state else {}
    s = Suite(a, endpoints, index, load_brokers(files))
    s.run()
    s.r.write(Path(a.out), {"export": str(export), "exportedAt": index.get("exportedAt", ""),
                            "endpoints": len(endpoints), "mqttBrokers": len(s.brokers)})
    t = s.r.totals()
    print(f"subscriptions: {t['pass']} pass, {t['fail']} fail, {t['skip']} skip")
    return 1 if t["fail"] else 0


if __name__ == "__main__":
    sys.exit(main())
