// Heavy mixed workload (T-5898): many concurrent large, complicated reads with writes beside them,
// against a broker dev/perf/heavy.sh seeded with dev/perf/k6-surfaces.js (SHAPE=seed, SURFACES=1).
// It is the load the buffered-answer pool of the memory budget (T-5895) has to carry without a
// refused read: whole-type temporal reads over every station, 1000-row geo and q pages, OGC pages
// of 10 000 features followed by their next link, OData $filter/$orderby pages and $batch, STA
// $expand, MCP summarize, with 100-entity upserts at a steady rate.
//
//   k6 run -e BROKER_URL=http://127.0.0.1:9471 -e READERS=30 -e DURATION=10m \
//     --summary-export heavy.json dev/perf/k6-heavy.js
//
// Env: BROKER_URL, READERS (30 looping readers), DURATION (10m), WRITE_RATE (5 upserts/s), KINDS,
//      ENTITIES, EVENTS, HISTORY_ENTITIES, HISTORY_DAYS (as at seed, so ids and windows match).
// Every read counts in heavy_ms (its time, the next page included) and, when the broker answers
// 5xx or a body the read cannot use, in heavy_failed; heavy_overloaded counts the 503s.
// Synthetic data only.

import http from "k6/http";
import { check } from "k6";
import { Trend, Counter, Rate } from "k6/metrics";
import { TYPES, countOf, request } from "./k6-surfaces.js";

const ROOT = __ENV.BROKER_URL || "http://127.0.0.1:9471";
const BASE = `${ROOT}/ngsi-ld/v1`;
const VIEW = `${ROOT}/x/views/load`;
const HISTORY_ENTITIES = Number(__ENV.HISTORY_ENTITIES || 500);
const MCP = "2026-07-28";
const JSON_HEADERS = { "Content-Type": "application/json" };
const q = encodeURIComponent;

export const options = {
  discardResponseBodies: false,
  scenarios: {
    readers: {
      executor: "constant-vus", exec: "reader",
      vus: Number(__ENV.READERS || 30), duration: __ENV.DURATION || "10m",
    },
    writers: {
      executor: "constant-arrival-rate", exec: "writer",
      rate: Number(__ENV.WRITE_RATE || 5), timeUnit: "1s",
      duration: __ENV.DURATION || "10m", preAllocatedVUs: 10, maxVUs: 40,
    },
  },
};

const heavyMs = new Trend("heavy_ms", true);
const failed = new Counter("heavy_failed");
const overloaded = new Counter("heavy_overloaded");
const reads = new Counter("heavy_reads");
const writeOk = new Rate("heavy_write_ok");

const stations = Math.min(HISTORY_ENTITIES, countOf("AirQualityObserved"));
const days = (from, n) => new Date(Date.parse("2026-09-01T00:00:00Z") + (from % 23) * 86400000 + n * 86400000).toISOString();

// [kind, method, url, body, follow next link]
function heavy(i) {
  const type = TYPES[i % (TYPES.length - 1)];
  const lon = 19 + (i % 800) / 1000;
  const lat = 48.5 + (i % 300) / 1000;
  switch (i % 11) {
    case 0: // newest ten values of every station at once
      return ["temporal-lastn-all", "GET", `${BASE}/temporal/entities?type=AirQualityObserved&attrs=pm25&lastN=10&timerel=after&timeAt=2026-09-01T00:00:00Z&limit=${Math.min(1000, stations)}`];
    case 1: // a week of every station
      return ["temporal-week-all", "GET", `${BASE}/temporal/entities?type=AirQualityObserved&attrs=pm25&timerel=between&timeAt=${days(i, 0)}&endTimeAt=${days(i, 7)}&limit=200`];
    case 2: // near with a q filter, a full page
      return ["geo-near-q", "GET", `${BASE}/entities?type=${type}&georel=${q("near;maxDistance==20000")}&geometry=Point&coordinates=${q(`[${lon},${lat}]`)}&q=${q(`level>=${i % 50};category!="c${i % 8}"`)}&limit=1000`];
    case 3: { // within a polygon, a full page
      const box = [[lon, lat], [lon + 0.2, lat], [lon + 0.2, lat + 0.2], [lon, lat + 0.2], [lon, lat]];
      return ["geo-within", "GET", `${BASE}/entities?type=${type}&georel=within&geometry=Polygon&coordinates=${q(JSON.stringify([box]))}&limit=1000`];
    }
    case 4: // the largest OGC page, then its next page
      return ["ogc-10000", "GET", `${VIEW}/ogc/features/collections/${type}/items?limit=10000`, null, true];
    case 5:
      return ["odata-filter", "GET", `${VIEW}/odata/v4/${type}?$filter=${q(`level ge ${i % 40} and category ne 'c${i % 8}'`)}&$top=1000&$count=true`];
    case 6:
      return ["odata-batch", "POST", `${VIEW}/odata/v4/$batch`, JSON.stringify({
        requests: TYPES.slice(0, 4).map((t, k) => ({ id: String(k), method: "get", url: `${t}?$top=1000&$orderby=level` })),
      })];
    case 7:
      return ["sta-expand", "GET", `${VIEW}/sta/v1.1/Things?$top=200&$skip=${(i % 20) * 200}&$expand=Datastreams&$count=true`];
    case 8:
      return ["mcp-summarize", "POST", `${VIEW}/mcp`, JSON.stringify({
        jsonrpc: "2.0", id: i, method: "tools/call",
        params: {
          name: "summarize", arguments: { type: [type] },
          _meta: {
            "io.modelcontextprotocol/protocolVersion": MCP,
            "io.modelcontextprotocol/clientInfo": { name: "k6-heavy", version: "1" },
            "io.modelcontextprotocol/clientCapabilities": {},
          },
        },
      })];
    case 9:
      return ["view-keyvalues", "GET", `${VIEW}/ngsi-ld/v1/entities?type=${type}&format=keyValues&limit=1000&offset=${(i % 10) * 1000}`];
    default: { // the batch query of three types inside a box with a q, a full page
      const box = [[lon, lat], [lon + 0.3, lat], [lon + 0.3, lat + 0.3], [lon, lat + 0.3], [lon, lat]];
      return ["query-post", "POST", `${BASE}/entityOperations/query?limit=1000`, JSON.stringify({
        type: "Query", entities: TYPES.slice(i % 3, (i % 3) + 3).map((t) => ({ type: t })),
        q: `level<${10 + (i % 80)}`, geoQ: { geometry: "Polygon", coordinates: [box], georel: "within" },
      })];
    }
  }
}

let shown = 0;
function judge(kind, r) {
  // a 503 is the pool's refusal (or another overload): the read the test exists to see never fail
  if (r.status === 503) overloaded.add(1, { kind });
  const mcpError = kind === "mcp-summarize" && String(r.body || "").includes('"isError":true');
  const ok = r.status === 200 && !mcpError;
  if (!ok) {
    failed.add(1, { kind, status: String(r.status) });
    // the first failures of each reader say why, in the k6 log beside the summary
    if (shown++ < 3) console.warn(`${kind}: ${r.status} ${String(r.body || "").slice(0, 300)}`);
  }
  return ok;
}

// KINDS (comma list) keeps only those reads: one kind alone shows what it costs the broker
const KINDS = (__ENV.KINDS || "").split(",").filter(Boolean);
export function reader() {
  let i = __ITER * 31 + __VU * 7919;
  while (KINDS.length && !KINDS.includes(heavy(i)[0])) i++;
  const [kind, method, url, body, follow] = heavy(i);
  const headers = kind === "mcp-summarize"
    ? { ...JSON_HEADERS, Accept: "application/json, text/event-stream", "MCP-Protocol-Version": MCP, "Mcp-Method": "tools/call", "Mcp-Name": "summarize" }
    : body ? JSON_HEADERS : {};
  const params = { headers, tags: { kind }, timeout: "60s" };
  const started = Date.now();
  let r = http.request(method, url, body || null, params);
  let ok = judge(kind, r);
  if (ok && follow) {
    const next = ((r.json("links") || []).find((l) => l.rel === "next") || {}).href;
    if (next) {
      r = http.get(next, params);
      ok = judge(kind, r);
    }
  }
  heavyMs.add(Date.now() - started, { kind });
  reads.add(1, { kind });
  check(r, { [`${kind} answered`]: () => ok });
}

export function writer() {
  const [method, url, body] = request("upsert-100", __ITER + 104729);
  const r = http.request(method, url, body, { headers: JSON_HEADERS, tags: { kind: "upsert-100" }, timeout: "60s" });
  writeOk.add(r.status === 201 || r.status === 204);
}
