// The request shapes other brokers publish, so the numbers compare:
//   query   GET /entities?type=Vehicle&limit=20&local=true   over 100 five-attribute entities
//           (local=true: the store answers alone, no registration fan-out)
//   query-q GET /entities?type=Vehicle&q=speed>100;brand=="Tesla"&limit=20&local=true
//           — the same page with a 4.9 q the store has to evaluate
//   query-deep  the same query at offset ENTITIES-20, the last page (T-4907)
//   query-regex GET /entities?type=Vehicle&q=brand~="Tes.*" (T-4907)
//   hfp     PATCH /entities/{id}/attrs with a new location per request, the
//           high-frequency position burst (T-4905)
//   retrieve GET /entities/{id}
//   upsert  POST /entityOperations/upsert, ten entities per request (5.6.8),
//           cycling over the seeded ids — Batch Upsert
//   subscribe POST /subscriptions (5.8.1), a new Subscription per request
//   update  PATCH /entities/{id}/attrs  (5.6.3, one Property per request),
//           spread over the same 100 entities — the write shape, which is
//           what takes the store's writer lock
//   facade  GET /x/example/things?kind=Vehicle — the reference façade, which
//           answers by driving the NGSI-LD router in process
//   facade-twin  GET /entities?type=Vehicle&options=keyValues — the request
//           that façade makes, so the pair prices the seam's round trip.
//           Both need a binary built with the plugin-example feature.
//   view    GET /x/views/{VIEW}/ngsi-ld/v1/entities?type=Vehicle&limit=20&local=true
//           — `query` through an endpoint view that shows everything
//   odata   GET /x/views/{VIEW}/odata/v4/Vehicle?$top=20 — the same page as an
//           OData EntitySet
//   mcp     POST /x/views/{VIEW}/mcp tools/call query_entities, 20 Vehicles —
//           the same page as an MCP tool call
//   ogc     GET /x/views/{VIEW}/ogc/features/collections/Vehicle/items?limit=20
//           — the same page as OGC API - Features GeoJSON
//           The four need a `full` binary with the view and the Tenant's
//           schema loaded (shapes.sh does it, BIN_SURFACES); ADR-0027
//           section 6 prices each against `query` on that binary.
// Closed model (constant VUs) on purpose here: this measures what the
// broker sustains at a fixed concurrency (c50, c200), which is what the
// published tables of other brokers report. k6-baseline.js keeps the open
// model for tail latency.
//
//   k6 run -e SHAPE=query -e VUS=50 -e DURATION=5s dev/perf/k6-shapes.js
//
// Env: BROKER_URL, SHAPE (query|query-q|retrieve|update|upsert|subscribe|facade|facade-twin|
//      view|odata|mcp|ogc), VIEW (perf),
//      VUS (50), DURATION (5s), TENANT (unset = default tenant), ENTITIES (100: how many
//      urn:ngsi-ld:Vehicle:shape:<n> are seeded, in upserts of 500).

import http from "k6/http";
import { check } from "k6";

const ROOT = __ENV.BROKER_URL || "http://localhost:9090";
const BASE = `${ROOT}/ngsi-ld/v1`;
const SHAPE = __ENV.SHAPE || "query";
const VIEW = `${ROOT}/x/views/${__ENV.VIEW || "perf"}`;
const MCP = "2026-07-28";
const mcpCall = JSON.stringify({
  jsonrpc: "2.0",
  id: 1,
  method: "tools/call",
  params: {
    name: "query_entities",
    arguments: { type: ["Vehicle"], limit: 20 },
    _meta: {
      "io.modelcontextprotocol/protocolVersion": MCP,
      "io.modelcontextprotocol/clientInfo": { name: "k6-shapes", version: "1" },
      "io.modelcontextprotocol/clientCapabilities": {},
    },
  },
});
const N = Number(__ENV.ENTITIES || 100);
const CHUNK = 500;
// JC_SUBJECT and JC_ROLES: the caller the platform's gateway would name
// (x-jc-subject, x-jc-roles), for a broker under ANTARES_POLICY=joinedcontext
// (dev/perf/platform-overhead.sh); unset, no such header is sent.
const headers = {
  ...(__ENV.TENANT ? { "NGSILD-Tenant": __ENV.TENANT } : {}),
  ...(__ENV.JC_SUBJECT ? { "x-jc-subject": __ENV.JC_SUBJECT, "x-jc-roles": __ENV.JC_ROLES || "" } : {}),
};
const json = { "Content-Type": "application/json", ...headers };

// SHAPE=seed seeds and measures nothing, SHAPE=unseed deletes the seed and
// measures nothing; SEED=0 skips the seed and leaves it standing, so a run
// measures its shape alone: k6 counts setup()'s requests in http_reqs and
// http_req_duration, and at ENTITIES=100000 the 200 upserts of the seed
// were most of a 5 s row's numbers (T-3735). dev/perf/shapes.sh seeds once
// per store, measures with SEED=0 and unseeds after its rows.
export const options = {
  scenarios: {
    shape:
      SHAPE === "seed" || SHAPE === "unseed"
        ? { executor: "shared-iterations", vus: 1, iterations: 1 }
        : {
            executor: "constant-vus",
            vus: Number(__ENV.VUS || 50),
            duration: __ENV.DURATION || "5s",
          },
  },
  summaryTrendStats: ["med", "p(99)", "max"],
  // seeding ENTITIES=100000 in upserts of 500 outlasts k6's 60 s default
  setupTimeout: "15m",
  teardownTimeout: "15m",
};

const brands = ["Mercedes", "Skoda", "Volvo", "Toyota", "Tesla"];

function vehicle(n, speed) {
  return {
    id: `urn:ngsi-ld:Vehicle:shape:${n}`,
    type: "Vehicle",
    brand: { type: "Property", value: brands[n % brands.length] },
    speed: { type: "Property", value: speed },
    mileage: { type: "Property", value: n * 1000, unitCode: "KMT" },
    colour: { type: "Property", value: "red" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [19.1 + (n % 1000) / 1000, 48.7] } },
  };
}

export function setup() {
  if (__ENV.SEED === "0" || SHAPE === "unseed") return;
  for (let n = 0; n < N; n += CHUNK) {
    const batch = [];
    for (let i = n; i < Math.min(n + CHUNK, N); i++) batch.push(vehicle(i, i % 130));
    const r = http.post(`${BASE}/entityOperations/upsert`, JSON.stringify(batch), { headers: json });
    if (r.status !== 201 && r.status !== 204) throw new Error(`seed ${n}: ${r.status} ${r.body}`);
  }
}

export default function () {
  if (SHAPE === "seed" || SHAPE === "unseed") return;
  let r;
  if (SHAPE === "facade") {
    r = http.get(`${ROOT}/x/example/things?kind=Vehicle`, { headers });
  } else if (SHAPE === "facade-twin") {
    // exactly what the façade asks the broker, so the difference is the seam
    r = http.get(`${BASE}/entities?type=Vehicle&options=keyValues`, { headers });
  } else if (SHAPE === "view") {
    r = http.get(`${VIEW}/ngsi-ld/v1/entities?type=Vehicle&limit=20&local=true`, { headers });
  } else if (SHAPE === "odata") {
    r = http.get(`${VIEW}/odata/v4/Vehicle?$top=20`, { headers });
  } else if (SHAPE === "ogc") {
    r = http.get(`${VIEW}/ogc/features/collections/Vehicle/items?limit=20`, { headers });
  } else if (SHAPE === "mcp") {
    r = http.post(`${VIEW}/mcp`, mcpCall, {
      headers: {
        ...json,
        Accept: "application/json, text/event-stream",
        "MCP-Protocol-Version": MCP,
        "Mcp-Method": "tools/call",
        "Mcp-Name": "query_entities",
      },
    });
    // a refused call is a 200 carrying isError: count only real answers
    check(r, { "200": (res) => res.status === 200 && !res.body.includes('"isError":true') });
    return;
  } else if (SHAPE === "retrieve") {
    r = http.get(`${BASE}/entities/urn:ngsi-ld:Vehicle:shape:${__ITER % N}`, { headers });
  } else if (SHAPE === "update") {
    // 5.6.3 Update Attributes: 204 on success. One Property, so the request
    // costs the store a write and almost nothing else — what is being
    // measured is the writer lock, not the payload.
    const body = JSON.stringify({ speed: { type: "Property", value: __ITER % 130 } });
    r = http.patch(`${BASE}/entities/urn:ngsi-ld:Vehicle:shape:${__ITER % N}/attrs`, body, {
      headers: { "Content-Type": "application/json", ...headers },
    });
    check(r, { "204": (res) => res.status === 204 });
    return;
  } else if (SHAPE === "query-q") {
    const q = encodeURIComponent('speed>100;brand=="Tesla"');
    r = http.get(`${BASE}/entities?type=Vehicle&q=${q}&limit=20&local=true`, { headers });
  } else if (SHAPE === "query-deep") {
    // the last page by offset: what a client paging to the end pays (T-4907)
    r = http.get(`${BASE}/entities?type=Vehicle&limit=20&offset=${Math.max(N - 20, 0)}&local=true`, { headers });
  } else if (SHAPE === "query-regex") {
    const q = encodeURIComponent('brand~="Tes.*"');
    r = http.get(`${BASE}/entities?type=Vehicle&q=${q}&limit=20&local=true`, { headers });
  } else if (SHAPE === "hfp") {
    // a position report per request, as a high-frequency vehicle feed writes
    // them (T-4905): the location index moves on every write
    const body = JSON.stringify({
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [19.1 + (__ITER % 1000) / 1000, 48.7] } },
    });
    r = http.patch(`${BASE}/entities/urn:ngsi-ld:Vehicle:shape:${(__VU * 7919 + __ITER) % N}/attrs`, body, {
      headers: { "Content-Type": "application/json", ...headers },
    });
    check(r, { "204": (res) => res.status === 204 });
    return;
  } else if (SHAPE === "upsert") {
    // 5.6.8 Batch Upsert of ten existing entities: 204 when all were updated
    const first = (__VU * 7919 + __ITER * 10) % N;
    const batch = [];
    for (let i = 0; i < 10; i++) batch.push(vehicle((first + i) % N, __ITER % 130));
    r = http.post(`${BASE}/entityOperations/upsert`, JSON.stringify(batch), { headers: json });
    check(r, { "2xx": (res) => res.status === 201 || res.status === 204 });
    return;
  } else if (SHAPE === "subscribe") {
    // 5.8.1 Create Subscription: 201 with the Location of the new one. On a
    // type nothing here writes, so the Subscriptions left behind never fire
    // on a later shape.
    const body = JSON.stringify({
      id: `urn:ngsi-ld:Subscription:shape:${__VU}:${__ITER}:${Date.now()}`,
      type: "Subscription",
      entities: [{ type: "ShapeNeverWritten" }],
      watchedAttributes: ["speed"],
      notification: { endpoint: { uri: "http://127.0.0.1:9/notify" } },
    });
    r = http.post(`${BASE}/subscriptions`, body, { headers: json });
    check(r, { "201": (res) => res.status === 201 });
    return;
  } else {
    r = http.get(`${BASE}/entities?type=Vehicle&limit=20&local=true`, { headers });
  }
  check(r, { "200": (res) => res.status === 200 });
}

export function teardown() {
  // a run that did not seed leaves the seed to the run after it
  if (SHAPE === "seed" || (__ENV.SEED === "0" && SHAPE !== "unseed")) return;
  for (let n = 0; n < N; n += CHUNK) {
    const ids = [];
    for (let i = n; i < Math.min(n + CHUNK, N); i++) ids.push(`urn:ngsi-ld:Vehicle:shape:${i}`);
    http.post(`${BASE}/entityOperations/delete`, JSON.stringify(ids), { headers: json });
  }
}
