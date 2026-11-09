"""Smoke suite `subscriptions`: NGSI-LD subscriptions over the Endpoint WebSockets and the edge
MQTT brokers, against production's transports as the export declares them.

   bins/jc/smoke/run subscriptions --base-url <url> --export <bundle> --out <dir> --state <state>
       [--per-class 1] [--wait 15] [--silence 5]
       [--mqtt-broker <project>/<broker>=mqtt://127.0.0.1:<port>]... [--mqtt-token-dir <dir>]

1. transport: every exported Endpoint's `/api/endpoint/{slug}/ws` without an upgrade answers as
   its transports say: the guide (426 Upgrade Required) when it lists `websocket` and is public, 401 when it is
   closed, 404 when it does not list it; an unknown slug 404s (the control). Production is read
   from the export alone: this suite opens no subscription on prod or dev.
2. delivery, per class (project x audience) of public Endpoints serving `websocket`, on a type its
   public grant opens without an id pattern or a q and a bound Policy lets a project ServiceAccount
   write: subscriber A gets the probe that ServiceAccount writes through the Endpoint (as the
   project's pipelines write) within --wait seconds with the TS 104 243 envelope
   (x-seq 1, x-subscription the created id); B on the same Endpoint gets it too (one fan-out hub
   per Endpoint, ADR-N-049); C, whose q excludes the probe, gets nothing; after A's x-unsubscribe
   a second write reaches B and not A (ws_delivery.rs, ws_hub.rs).
3. outside the grant: D on another tenant's Endpoint, subscribed to the same type, gets nothing;
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

--realm-secrets-dir holds the client secrets of the exported realm (`$(env:NAME)` -> file NAME):
the probe writer's token comes from it; without it part 2 is skipped. Probes are deleted at the
end (JC-Reason given)."""

from __future__ import annotations

import argparse
import json
import re
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

MARK = "smokeSubsMark"
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


WRITE_OPS = {"upsertBatch", "createBatch", "createEntity", "upsertOps", "createOps", "writeOps"}


def writer_of(e: ex.Endpoint, st: str) -> str | None:
    """The ServiceAccount a bound Policy lets write type `st` whole on the Endpoint's space (the
    project's pipelines, as they write), or None."""
    for g in e.grants:
        if g.assignee.get("kind") != "serviceAccount" or g.narrowed or not g.operations & WRITE_OPS:
            continue
        if not g.selectors or any(x.type in (st, "*") and x.pattern is None for x in g.selectors):
            return str(g.assignee.get("id"))
    return None


def plan(endpoints, index, per_class: int):
    """(chosen [(class, endpoint, short type, iri, writer)], skipped [(endpoint, why)]) for part 2."""
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
            writable = {t: types[t] for t in types if writer_of(e, t)}
            if not writable:
                skipped.append((e, "no ServiceAccount of the project may write a type the public grant opens"))
                continue
            st = max(writable, key=lambda t: (not writable[t].get("expires"), writable[t]["count"]))
            chosen.append((f"{k[0]}/{k[1]}", e, st, writable[st]["iri"], writer_of(e, st)))
            n += 1
    return chosen, skipped


def load_clients(export: Path) -> dict:
    """{client id: (realm, the $(env:NAME) of its secret)} of the exported realms."""
    out = {}
    for f in sorted((export / "keycloak").glob("realm-*.json")):
        try:
            realm = json.loads(f.read_text())
        except ValueError:
            continue
        for c in realm.get("clients") or []:
            m = re.fullmatch(r"\$\(env:([A-Z0-9_]+)\)", str(c.get("secret") or ""))
            if m:
                out[c["clientId"]] = (realm.get("realm") or f.stem[6:], m.group(1))
    return out


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
    def __init__(self, a, endpoints, index, brokers, clients=None):
        self.a, self.endpoints, self.index, self.brokers = a, endpoints, index, brokers
        self.clients = clients or {}
        self.http = Http(a.base_url, timeout=a.timeout)
        self.r = Results("subscriptions", a.base_url)
        self.tokens: dict[str, str] = {}
        self.probes: list[tuple[ex.Endpoint, str, str]] = []

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
                # The gateway's guide (handlers/ws.rs guide()): how to open the socket, as a 426.
                want, ok = "426 guide", st == 426 and isinstance(as_json(body), dict)
            self.r.check(f"transport.{e.slug}", f"{e.project}/{e.name}", f"GET {ws_path(e.slug)}", want,
                         f"{st} {short(body, 100)}", ok)

    # ------------------------------------------------------------ probes

    def token_for(self, project: str, account: str) -> tuple[str | None, str]:
        """A client-credentials token of the project's ServiceAccount, its secret read from the
        rehearsal's realm secrets dir (never printed); or (None, why)."""
        client = f"{project}-{account}"
        if client in self.tokens:
            return self.tokens[client], ""
        if client not in self.clients:
            return None, f"no client {client} in the exported realms"
        realm, var = self.clients[client]
        f = Path(self.a.realm_secrets_dir or "/nonexistent") / var
        if not f.is_file():
            return None, f"no secret file {var} in --realm-secrets-dir"
        form = urllib.parse.urlencode({"grant_type": "client_credentials", "client_id": client,
                                       "client_secret": f.read_text().strip()}).encode()
        st, _, raw = self.http.call("POST", f"/id/realms/{realm}/protocol/openid-connect/token", body=form,
                                    headers={"Content-Type": "application/x-www-form-urlencoded"})
        tok = (as_json(raw) or {}).get("access_token") if st == 200 else None
        if not tok:
            return None, f"token of {client} in realm {realm}: {st}"
        self.tokens[client] = tok
        return tok, ""

    def door(self, e: ex.Endpoint, token: str) -> ex.Endpoint | None:
        """The Endpoint of e's space the token is bound to (its `aud` holds Endpoint slugs: the
        gateway refuses any other with `token is not bound to this resource`), e itself first."""
        aud = jwt_claims(token).get("aud") or []
        aud = {aud} if isinstance(aud, str) else set(aud)
        return next((x for x in [e, *sorted(self.endpoints, key=lambda x: x.slug)]
                     if x.tenant == e.tenant and x.slug in aud), None)

    def template(self, e: ex.Endpoint, st: str) -> tuple[dict | None, str]:
        """The probe's body but its id, from the type's sample as the public reads it on `e`: its
        short type (the write guard wants the URN's type segment, write_guard.rs
        check_identifier), its @context (so the type keeps its IRI) and one Property it shows (a
        change of no shown attribute notifies no view subscriber); or (None, why)."""
        sample = ((self.index.get("tenants") or {}).get(e.tenant, {}).get(st) or {}).get("sample")
        if not sample:
            return None, f"no sample {st} in the export"
        code, _, raw = self.http.call("GET", f"/api/endpoint/{e.slug}/ngsi-ld/v1/entities/"
                                      + urllib.parse.quote(sample, safe=""), headers={"Accept": "application/ld+json"})
        ent = as_json(raw) if code == 200 else None
        if not isinstance(ent, dict) or "@context" not in ent:
            return None, f"GET the sample {sample}: {code} {short(raw, 80)}"
        attr = next((k for k, v in ent.items() if isinstance(v, dict) and v.get("type") == "Property"
                     and "value" in v), None)
        if not attr:
            return None, f"the sample {sample} shows no Property"
        return {"type": st, attr: {"type": "Property", "value": ent[attr]["value"]}, "@context": ent["@context"]}, ""

    def write(self, e: ex.Endpoint, eid: str, body: dict, token: str) -> tuple[int, bytes]:
        """The probe, written as the project's pipelines write: through a write Endpoint of the
        space, a batch in JSON-LD."""
        st, _, raw = self.http.call("POST", f"/api/endpoint/{e.slug}/ngsi-ld/v1/entityOperations/upsert?options=update",
                                    token=token, body=[{"id": eid, **body}],
                                    headers={"Content-Type": "application/ld+json"})
        if st in (200, 201, 204):
            self.probes.append((e, eid, token))
        return st, raw

    def cleanup(self):
        """Deletes the probes; a probe left behind (a writer with no delete grant) is a named row,
        never silent, so the rehearsal's owner knows what to remove."""
        left = []
        for e, eid, token in self.probes:
            st, _, _ = self.http.call("POST", f"/api/endpoint/{e.slug}/ngsi-ld/v1/entityOperations/delete", token=token,
                                      headers={"JC-Reason": "jc-smoke subscriptions probe"}, body=[eid])
            if st not in (200, 204):
                left.append(f"{e.project}/{e.name} {eid} ({st})")
        if left:
            self.r.skip("cleanup.probes", "the rehearsal", "POST .../entityOperations/delete (JC-Reason)",
                        "every probe deleted", f"{len(left)} probe(s) left: the writer's Policy grants no delete: "
                        + "; ".join(left[:10]))

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

    def deliver(self, cls: str, e: ex.Endpoint, st: str, iri: str, writer: str, door: ex.Endpoint,
                other: ex.Endpoint | None):
        t, w = f"{e.project}/{e.name}", self.a.wait
        tag = secrets.token_hex(4)
        probe, probe2 = f"urn:ngsi-ld:{st}:smoke-subs-{tag}", f"urn:ngsi-ld:{st}:smoke-subs-{tag}-2"
        sub = {"type": "Subscription", "entities": [{"type": iri}]}
        token = self.tokens[f"{e.project}-{writer}"]
        body, why = self.template(e, st)
        if not body:
            self.r.skip(f"deliver.{e.slug}", t, f"GET a sample {st} on {ws_path(e.slug)[:-3]}", "x-notification",
                        f"no probe to write: {why}")
            return
        socks = []
        try:
            a, why = self.socket(e)
            if not a:
                self.r.check(f"deliver.{e.slug}", t, f"WS {ws_path(e.slug)}", "upgrade 101 and x-welcome", why, False,
                             f"class {cls}, type {st}")
                return
            socks.append(a)
            st_a, id_a, msg_a = self.subscribe(a, sub, "a")
            b, _ = self.socket(e)
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
            if st_a != 201:
                self.r.check(f"deliver.{e.slug}", t, "x-subscribe", "x-status 201", f"{st_a} {msg_a}", False,
                             f"class {cls}, type {st}")
                return
            time.sleep(self.a.settle)  # the hub's broker subscription is made after the 201
            wst, wraw = self.write(door, probe, body, token)
            if wst not in (200, 201, 204):
                self.r.check(f"deliver.{e.slug}", t, f"POST {ws_path(door.slug)[:-3]}/ngsi-ld/v1/entityOperations/upsert "
                             f"as {e.project}-{writer}", "the probe is written", f"{wst} {short(wraw)}", False,
                             "the probe write failed: nothing to deliver")
                return
            started = time.monotonic()
            n = a.until(carries(probe), w)
            took = time.monotonic() - started
            got = bool(n)
            self.r.check(f"deliver.{e.slug}", t, f"WS {ws_path(e.slug)} x-subscribe {st}; {e.project}-{writer} writes "
                         f"{probe} through {door.project}/{door.name}",
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
            # A leaves; the hub keeps serving B.
            try:
                a.send({"metadata": {"x-op": "x-unsubscribe", "x-id": "u", "x-subscription": id_a}})
                un = a.until(lambda m: wsclient.op(m) == "x-result" and (m.get("metadata") or {}).get("x-id") == "u", w)
            except (OSError, wsclient.Closed):
                un = None
            self.write(door, probe2, body, token)
            nb2 = b.until(carries(probe2), w) if nb else None
            came = self.silent(a, carries(probe2), self.a.silence)
            un_st = (un or {}).get("metadata", {}).get("x-status")
            self.r.check(f"hub.{e.slug}.unsubscribe", t, "x-unsubscribe A, then a second probe",
                         "A: 204 and nothing; B: notified",
                         f"A {un_st}, {came or 'nothing'}; B {'notified' if nb2 else 'nothing'}",
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
        if not self.a.realm_secrets_dir:
            self.r.skip("deliver", "public Endpoints serving websocket", "WS + probe write", "x-notification",
                        "no --realm-secrets-dir: the project's ServiceAccount cannot write the probe")
            return
        cands, skipped = plan(self.endpoints, self.index, 10 ** 6)
        chosen, taken = [], defaultdict(int)
        for cls, e, st, iri, writer in cands:
            if taken[cls] >= self.a.per_class:
                continue
            token, why = self.token_for(e.project, writer)
            door = self.door(e, token) if token else None
            if not door:
                why = why or f"the token of {e.project}-{writer} is bound to no Endpoint of space {e.space} (aud)"
                skipped.append((e, f"the probe writer cannot write here: {why}"))
                continue
            chosen.append((cls, e, st, iri, writer, door))
            taken[cls] += 1
        for e, why in skipped[:50]:
            self.r.skip(f"deliver.{e.slug}", f"{e.project}/{e.name}", "WS + probe write", "x-notification", why)
        for i, (cls, e, st, iri, writer, door) in enumerate(chosen):
            other = next((x[1] for x in chosen[i + 1:] + chosen[:i] if x[1].tenant != e.tenant), None)
            self.deliver(cls, e, st, iri, writer, door, other)

    # ------------------------------------------------------------ 4. MQTT

    def mqtt(self):
        st, _, body = self.http.call("POST", "/internal/mqtt/auth",
                                     body={"clientid": "smoke", "username": "smoke", "password": "x", "ipaddr": "127.0.0.1"})
        # Whatever the public base answers (404, or the Portal's page), never a CONNECT decision.
        decided = isinstance(as_json(body), dict) and "result" in as_json(body)
        self.r.check("mqtt.auth-not-public", "gateway", "POST /internal/mqtt/auth on the public base",
                     "no CONNECT decision (internal listener only)", f"{st} {short(body, 80)}", not decided and st != 0)
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
            self.cleanup()


def main(argv=None):
    p = argparse.ArgumentParser(prog="run subscriptions", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--base-url", required=True)
    p.add_argument("--export", required=True, help="the export bundle the rehearsal was built from")
    p.add_argument("--out", required=True)
    p.add_argument("--realm-secrets-dir", help="the rehearsal's realm secrets ($REHEARSAL_HOME/<env>/secrets/realm): "
                   "the project ServiceAccount's client secret that writes the probe")
    p.add_argument("--state", help="unused (accepted for the common runner)")
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
    index = ex.load_index(export, endpoints, Path(a.cache)) if a.realm_secrets_dir else {}
    s = Suite(a, endpoints, index, load_brokers(files), load_clients(export))
    s.run()
    s.r.write(Path(a.out), {"export": str(export), "exportedAt": index.get("exportedAt", ""),
                            "endpoints": len(endpoints), "mqttBrokers": len(s.brokers)})
    t = s.r.totals()
    print(f"subscriptions: {t['pass']} pass, {t['fail']} fail, {t['skip']} skip")
    return 1 if t["fail"] else 0


if __name__ == "__main__":
    sys.exit(main())
