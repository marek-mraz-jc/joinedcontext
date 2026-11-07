// One load shape per API surface (T-4889), against a broker that dev/perf/surfaces.sh seeded with
// SHAPE=seed: ENTITIES synthetic entities over ten types, every one with a Point, AirQualityObserved
// carrying a HISTORY_DAYS history of pm25 on its first HISTORY_ENTITIES entities, and EVENTS Events
// for the iCalendar feed. On a platform build the seed also puts view `load` (every type) and the
// default Tenant's LinkML schema, which OData, OGC Features, SensorThings, iCalendar, MCP and the
// view's WebSocket read.
//
//   k6 run -e SHAPE=seed -e BROKER_URL=http://127.0.0.1:9471 -e SURFACES=1 dev/perf/k6-surfaces.js
//   k6 run -e SHAPE=ngsi-q -e RATE=200 -e DURATION=30s dev/perf/k6-surfaces.js
//
// Open model (constant arrival rate): a shape that cannot hold RATE shows it as dropped
// iterations, and its latency is measured at the rate it was asked, not at the rate it managed.
// dev/perf/surfaces-budgets.json holds each shape's rate and its budget; surfaces_gate.py judges.
//
// Env: BROKER_URL, SHAPE, RATE (50), DURATION (30s), VUS (RATE), ENTITIES (100000), EVENTS (5000),
//      HISTORY_ENTITIES (500), HISTORY_DAYS (30), SURFACES (1 = put view and schema at seed),
//      SUBSCRIBERS (1000, ws-fanout), TENANT (unset = default).
// Synthetic data only: no real place, person or device is named.

import http from "k6/http";
import { check } from "k6";
import { Trend, Rate, Counter } from "k6/metrics";
import { WebSocket } from "k6/websockets";
import { setTimeout } from "k6/timers";
import exec from "k6/execution";

const ROOT = __ENV.BROKER_URL || "http://127.0.0.1:9471";
const BASE = `${ROOT}/ngsi-ld/v1`;
const VIEW = `${ROOT}/x/views/load`;
const SHAPE = __ENV.SHAPE || "ngsi-type";
const RATE = Number(__ENV.RATE || 50);
const DURATION = __ENV.DURATION || "30s";
const ENTITIES = Number(__ENV.ENTITIES || 100000);
const EVENTS = Math.min(Number(__ENV.EVENTS || 5000), ENTITIES);
const HISTORY_ENTITIES = Number(__ENV.HISTORY_ENTITIES || 500);
const HISTORY_DAYS = Number(__ENV.HISTORY_DAYS || 30);
const SUBSCRIBERS = Number(__ENV.SUBSCRIBERS || 1000);
const LANES = 20;
const MCP = "2026-07-28";
const tenant = __ENV.TENANT ? { "NGSILD-Tenant": __ENV.TENANT } : {};
const JSON_HEADERS = { "Content-Type": "application/json", ...tenant };

// Ten types; Event takes EVENTS, the other nine share the rest. Every entity has a Point in a
// 1 x 0.5 degree box, a numeric `level` 0..99 and a `category` of eight values.
export const TYPES = [
  "AirQualityObserved", "Vehicle", "Building", "ParkingSpot", "Streetlight",
  "WeatherObserved", "WaterQualityObserved", "TrafficFlowObserved", "Device", "Event",
];
const OTHERS = TYPES.length - 1;
export function countOf(type) {
  if (type === "Event") return EVENTS;
  const rest = ENTITIES - EVENTS;
  const i = TYPES.indexOf(type);
  return Math.floor(rest / OTHERS) + (i < rest % OTHERS ? 1 : 0);
}
const urn = (type, n) => `urn:ngsi-ld:${type}:load:${n}`;
const HISTORY_START = Date.parse("2026-09-01T00:00:00Z");
const HISTORY_STEP_MS = 6 * 3600 * 1000; // four instances a day

export function entity(type, n) {
  const e = {
    id: urn(type, n),
    type,
    name: { type: "Property", value: `${type} ${n}` },
    level: { type: "Property", value: n % 100 },
    category: { type: "Property", value: `c${n % 8}` },
    location: {
      type: "GeoProperty",
      value: { type: "Point", coordinates: [19 + (n % 1000) / 1000, 48.5 + (Math.floor(n / 1000) % 500) / 1000] },
    },
  };
  if (type === "TrafficFlowObserved") e.lane = { type: "Property", value: n % LANES };
  // SensorThings maps a Datastream from a current Attribute: a history entity holds its newest
  // pm25 too, or sta-observations reads a Datastream that does not exist (404)
  if (type === "AirQualityObserved" && n < HISTORY_ENTITIES) {
    e.pm25 = { type: "Property", value: 5 + ((n * 7) % 60), observedAt: new Date(HISTORY_START + HISTORY_DAYS * 86400000).toISOString() };
  }
  if (type === "Event") {
    const start = new Date(HISTORY_START + (n % 60) * 86400000 + (n % 24) * 3600000);
    e.startDate = { type: "Property", value: { "@type": "DateTime", "@value": start.toISOString() } };
    e.endDate = { type: "Property", value: { "@type": "DateTime", "@value": new Date(start.getTime() + 7200000).toISOString() } };
  }
  return e;
}

// The LinkML schema of the default Tenant: OGC Features and OData read their collections and
// entity sets from it, so every type and slot the load reads is named.
export function schema() {
  const slots = {
    name: { range: "string" }, level: { range: "integer" }, category: { range: "string" },
    lane: { range: "integer" }, sentAt: { range: "integer" }, pm25: { range: "float" }, startDate: { range: "datetime" },
    endDate: { range: "datetime" },
    location: {
      slot_uri: "https://uri.etsi.org/ngsi-ld/location",
      annotations: { ngsi_ld_kind: { tag: "ngsi_ld_kind", value: "GeoProperty" } },
    },
  };
  const classes = {};
  for (const type of TYPES) {
    const own = ["name", "level", "category", "location"];
    if (type === "TrafficFlowObserved") own.push("lane", "sentAt");
    if (type === "AirQualityObserved") own.push("pm25");
    if (type === "Event") own.push("startDate", "endDate");
    classes[type] = { slots: own };
  }
  return {
    id: "https://example.org/load", name: "load", default_prefix: "d",
    prefixes: { d: { prefix_prefix: "d", prefix_reference: "https://uri.etsi.org/ngsi-ld/default-context/" } },
    classes, slots,
  };
}

function put(path, body) {
  const r = http.put(`${ROOT}${path}`, JSON.stringify(body), { headers: JSON_HEADERS });
  if (r.status >= 300) throw new Error(`PUT ${path}: ${r.status} ${r.body}`);
}

function seed() {
  if (__ENV.SURFACES === "1") {
    put("/q/schemas/default", schema());
    put("/q/views/load", {
      id: "load", tenant: "default", selector: TYPES,
      rules: [{ types: TYPES, attributes: ["name", "level", "category", "lane", "sentAt", "pm25", "startDate", "endDate", "location"] }],
      // a view serves a calendar only for the outputs it names (T-5208): `events` of the sdm-event preset
      representations: { ical: [{ name: "events", sources: [{}] }] },
      operations: ["query_entities", "retrieve_entity", "retrieve_entity_types", "retrieve_attributes", "temporal_query", "temporal_retrieve", "create_subscription", "delete_subscription"],
    });
  }
  for (const type of TYPES) {
    const total = countOf(type);
    for (let n = 0; n < total; n += 500) {
      const batch = [];
      for (let i = n; i < Math.min(n + 500, total); i++) batch.push(entity(type, i));
      const r = http.post(`${BASE}/entityOperations/upsert`, JSON.stringify(batch), { headers: JSON_HEADERS });
      if (r.status !== 201 && r.status !== 204) throw new Error(`seed ${type} ${n}: ${r.status} ${r.body}`);
    }
  }
  // 30 days of pm25, four instances a day, per history entity (5.6.11 Create or Update Temporal)
  const steps = Math.floor((HISTORY_DAYS * 86400000) / HISTORY_STEP_MS);
  for (let n = 0; n < Math.min(HISTORY_ENTITIES, countOf("AirQualityObserved")); n++) {
    const instances = [];
    for (let s = 0; s < steps; s++) {
      instances.push({
        type: "Property", value: 5 + ((n * 7 + s * 13) % 60),
        observedAt: new Date(HISTORY_START + s * HISTORY_STEP_MS).toISOString(),
      });
    }
    const body = { id: urn("AirQualityObserved", n), type: "AirQualityObserved", pm25: instances };
    const r = http.post(`${BASE}/temporal/entities`, JSON.stringify(body), { headers: JSON_HEADERS });
    if (r.status !== 201 && r.status !== 204) throw new Error(`history ${n}: ${r.status} ${r.body}`);
  }
}

export const options =
  SHAPE === "seed"
    ? { scenarios: { seed: { executor: "shared-iterations", vus: 1, iterations: 1, maxDuration: "60m" } } }
    : {
        scenarios: Object.assign(
          {
            load: {
              executor: "constant-arrival-rate", rate: RATE, timeUnit: "1s", duration: DURATION,
              preAllocatedVUs: Number(__ENV.VUS || Math.max(10, RATE)), maxVUs: Math.max(50, RATE * 4),
              exec: SHAPE === "ws-fanout" ? "writer" : "load",
              // the subscribers connect and subscribe before the first write
              startTime: SHAPE === "ws-fanout" ? "20s" : "0s",
            },
          },
          SHAPE === "ws-fanout"
            ? { subscribers: { executor: "per-vu-iterations", vus: SUBSCRIBERS, iterations: 1, startTime: "0s", maxDuration: "30m", exec: "subscriber" } }
            : {},
        ),
        summaryTrendStats: ["avg", "med", "p(95)", "p(99)", "max"],
        discardResponseBodies: SHAPE !== "upsert-100",
      };

// A request of the shape: method, URL, body. `i` spreads the reads over the seeded ids and pages.
export function request(shape, i) {
  const type = TYPES[i % OTHERS];
  const n = i % Math.max(1, countOf(type));
  const lon = 19 + (i % 900) / 1000;
  const lat = 48.5 + (i % 400) / 1000;
  const page = (i % 50) * 20;
  const q = encodeURIComponent;
  switch (shape) {
    case "ngsi-id": return ["GET", `${BASE}/entities/${urn(type, n)}`];
    case "ngsi-type": return ["GET", `${BASE}/entities?type=${type}&limit=20&offset=${page}`];
    case "ngsi-q": return ["GET", `${BASE}/entities?type=${type}&q=${q(`level>=${i % 90};level<${(i % 90) + 10};category=="c${i % 8}"`)}&limit=20`];
    case "ngsi-near":
      return ["GET", `${BASE}/entities?type=AirQualityObserved&georel=${q("near;maxDistance==500")}&geometry=Point&coordinates=${q(`[${lon},${lat}]`)}&limit=20`];
    case "ngsi-within": {
      const box = [[lon, lat], [lon + 0.02, lat], [lon + 0.02, lat + 0.02], [lon, lat + 0.02], [lon, lat]];
      return ["GET", `${BASE}/entities?type=AirQualityObserved&georel=within&geometry=Polygon&coordinates=${q(JSON.stringify([box]))}&limit=20`];
    }
    case "ngsi-attrs": return ["GET", `${BASE}/entities?type=${type}&attrs=name,level&limit=100&offset=${page}`];
    case "temporal-lastn":
      return ["GET", `${BASE}/temporal/entities/${urn("AirQualityObserved", i % HISTORY_ENTITIES)}?attrs=pm25&lastN=10&timerel=after&timeAt=2026-09-01T00:00:00Z`];
    case "temporal-between": {
      const from = new Date(HISTORY_START + (i % 23) * 86400000).toISOString();
      const to = new Date(HISTORY_START + ((i % 23) + 7) * 86400000).toISOString();
      return ["GET", `${BASE}/temporal/entities/${urn("AirQualityObserved", i % HISTORY_ENTITIES)}?attrs=pm25&timerel=between&timeAt=${from}&endTimeAt=${to}`];
    }
    case "upsert-100": {
      const batch = [];
      const first = (i * 100) % Math.max(1, countOf("Vehicle"));
      for (let k = 0; k < 100; k++) batch.push(entity("Vehicle", (first + k) % countOf("Vehicle")));
      return ["POST", `${BASE}/entityOperations/upsert`, JSON.stringify(batch)];
    }
    case "view-type": return ["GET", `${VIEW}/ngsi-ld/v1/entities?type=${type}&limit=20&offset=${page}`];
    // every attribute name the view shows (T-4934: answered from the attribute catalogue, 13-19 s on dev as a fold)
    case "view-attributes": return ["GET", `${VIEW}/ngsi-ld/v1/attributes`];
    case "odata":
      return ["GET", `${VIEW}/odata/v4/${type}?$filter=${q(`level ge ${i % 90} and level lt ${(i % 90) + 10}`)}&$orderby=level%20desc&$top=20&$count=true`];
    case "ogc-items": {
      const bbox = `${lon},${lat},${(lon + 0.05).toFixed(3)},${(lat + 0.05).toFixed(3)}`;
      return ["GET", `${VIEW}/ogc/features/collections/AirQualityObserved/items?bbox=${bbox}&filter=${q(`level>${i % 50}`)}&filter-lang=cql2-text&limit=20`];
    }
    // the first page; load() follows its `next` link, as a GIS client pages (OGC paging is a cursor)
    case "ogc-page": return ["GET", `${VIEW}/ogc/features/collections/${type}/items?limit=50`];
    case "sta-things": return ["GET", `${VIEW}/sta/v1.1/Things?$top=20&$skip=${page}&$expand=Datastreams&$count=true`];
    case "sta-observations": {
      const ds = q(`'${urn("AirQualityObserved", i % HISTORY_ENTITIES)}/pm25'`);
      const from = new Date(HISTORY_START + (i % 23) * 86400000).toISOString();
      return ["GET", `${VIEW}/sta/v1.1/Datastreams(${ds})/Observations?$filter=${q(`phenomenonTime ge ${from}`)}&$top=100&$orderby=${q("phenomenonTime desc")}`];
    }
    case "ical": return ["GET", `${VIEW}/ical/v1/events.ics?from=2026-09-01&to=2026-11-30`];
    case "mcp":
      return ["POST", `${VIEW}/mcp`, JSON.stringify({
        jsonrpc: "2.0", id: i, method: "tools/call",
        params: {
          name: "query_entities", arguments: { type: [type], q: `level>=${i % 90}`, limit: 20 },
          _meta: {
            "io.modelcontextprotocol/protocolVersion": MCP,
            "io.modelcontextprotocol/clientInfo": { name: "k6-surfaces", version: "1" },
            "io.modelcontextprotocol/clientCapabilities": {},
          },
        },
      })];
    default: throw new Error(`unknown SHAPE ${shape}`);
  }
}

const answered = new Rate("shape_ok");
export function load() {
  if (SHAPE === "seed") {
    // a thrown error only ends the iteration and k6 still exits 0: a seed that did not land
    // must fail the run, or every shape measures an empty broker
    try {
      seed();
    } catch (err) {
      exec.test.abort(String(err));
    }
    return;
  }
  const [method, url, body] = request(SHAPE, __ITER + __VU * 7919);
  const headers = SHAPE === "mcp"
    ? { ...JSON_HEADERS, Accept: "application/json, text/event-stream", "MCP-Protocol-Version": MCP, "Mcp-Method": "tools/call", "Mcp-Name": "query_entities" }
    : body ? JSON_HEADERS : tenant;
  const params = { headers, tags: { shape: SHAPE }, timeout: "10s" };
  if (SHAPE === "ogc-page") params.responseType = "text";
  let r = http.request(method, url, body || null, params);
  if (SHAPE === "ogc-page" && r.status === 200) {
    const next = ((r.json("links") || []).find((l) => l.rel === "next") || {}).href;
    r = next ? http.get(next, { headers, tags: { shape: SHAPE }, timeout: "10s" }) : { status: 0 };
  }
  // upsert answers 201/204; an MCP refusal is a 200 carrying isError, so the MCP check reads it
  const ok = SHAPE === "upsert-100"
    ? r.status === 201 || r.status === 204
    : r.status === 200 && (SHAPE !== "mcp" || !String(r.body || "").includes('"isError":true'));
  answered.add(ok, { shape: SHAPE });
  check(r, { [`${SHAPE} answered`]: () => ok });
}
export default load;

// ws-fanout: SUBSCRIBERS sockets on the view's WebSocket, subscriber k watching TrafficFlowObserved
// lane k % 20, while the writer patches RATE entities a second with the time it wrote. Each write
// reaches SUBSCRIBERS / 20 sockets; the trend is write-to-delivery time.
const fanoutMs = new Trend("ws_fanout_ms", true);
const wsSubscribed = new Rate("ws_subscribed");
const wsNotified = new Counter("ws_notifications");
const wsGaps = new Counter("ws_gaps");
// what the writes owe: each write reaches the SUBSCRIBERS / 20 sockets on its lane
const wsExpected = new Counter("ws_expected");
export function subscriber() {
  // one iteration per subscriber, numbered 0..SUBSCRIBERS-1: every lane gets SUBSCRIBERS / 20
  const lane = exec.scenario.iterationInTest % LANES;
  const socket = new WebSocket(`${VIEW.replace(/^http/, "ws")}/ws`, ["ngsi-ld-ws"]);
  let done = false;
  socket.onopen = () => {
    socket.send(JSON.stringify({
      metadata: { "x-op": "x-subscribe", "x-id": "load" },
      body: { type: "Subscription", entities: [{ type: "TrafficFlowObserved" }], q: `lane==${lane}`, watchedAttributes: ["level", "sentAt"] },
    }));
    // hold the socket while the writer runs, plus the time a late notification needs
    setTimeout(() => socket.close(), (secondsOf(DURATION) + 35) * 1000);
  };
  socket.onmessage = (event) => {
    let message;
    try {
      message = JSON.parse(event.data);
    } catch (_) {
      return;
    }
    const meta = message.metadata || {};
    if (meta["x-op"] === "x-result" && meta["x-id"] === "load" && !done) {
      done = true;
      const status = Number(meta["x-status"]);
      wsSubscribed.add(status >= 200 && status < 300);
    } else if (meta["x-op"] === "x-notification") {
      for (const e of (message.body && message.body.data) || []) {
        const sent = e.sentAt && (e.sentAt.value ?? e.sentAt);
        if (typeof sent === "number") fanoutMs.add(Date.now() - sent);
        wsNotified.add(1);
      }
    } else if (meta["x-op"] === "x-gap") {
      wsGaps.add(1);
    }
  };
  socket.onclose = () => {
    if (!done) wsSubscribed.add(false);
  };
}

export function writer() {
  const n = (__ITER + __VU * 7919) % countOf("TrafficFlowObserved");
  const body = JSON.stringify({ level: { type: "Property", value: __ITER % 100 }, sentAt: { type: "Property", value: Date.now() } });
  const r = http.patch(`${BASE}/entities/${urn("TrafficFlowObserved", n)}/attrs`, body, { headers: JSON_HEADERS, tags: { shape: SHAPE } });
  answered.add(r.status === 204, { shape: SHAPE });
  if (r.status === 204) wsExpected.add(SUBSCRIBERS / LANES);
}

function secondsOf(text) {
  const m = /^(\d+)(s|m)$/.exec(text);
  if (!m) throw new Error(`DURATION '${text}': use <n>s or <n>m`);
  return Number(m[1]) * (m[2] === "m" ? 60 : 1);
}
