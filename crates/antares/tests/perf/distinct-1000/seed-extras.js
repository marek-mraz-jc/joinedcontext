// What the bulk seed (seed.py) leaves to the API: the default Tenant's LinkML schema and the view
// `load` of dev/perf/k6-surfaces.js, and 30 days of pm25 (four instances a day) on the first
// HISTORY_ENTITIES AirQualityObserved, as k6-surfaces.js's seed writes them.
//
//   k6 run -e BROKER_URL=http://127.0.0.1:9471 tests/perf/distinct-1000/seed-extras.js
//
// TENANT (default) and VIEW (load) name the Tenant and the view; the gateway leg seeds the Tenant
// of its Context Space and names the view after its Endpoint's slug, as the Portal's reconciler does.

import http from "k6/http";
import { schema, TYPES } from "../../../dev/perf/k6-surfaces.js";

const ROOT = __ENV.BROKER_URL || "http://127.0.0.1:9471";
const HISTORY_ENTITIES = Number(__ENV.HISTORY_ENTITIES || 500);
const HISTORY_START = Date.parse("2026-09-01T00:00:00Z");
const STEP_MS = 6 * 3600 * 1000;
const TENANT = __ENV.TENANT || "default";
const VIEW = __ENV.VIEW || "load";
const HEADERS = { "Content-Type": "application/json" };
const WRITE = TENANT === "default" ? HEADERS : { ...HEADERS, "NGSILD-Tenant": TENANT };

export const options = { scenarios: { seed: { executor: "shared-iterations", vus: 1, iterations: 1, maxDuration: "30m" } } };

function put(path, body) {
  const r = http.put(`${ROOT}${path}`, JSON.stringify(body), { headers: HEADERS });
  if (r.status >= 300) throw new Error(`PUT ${path}: ${r.status} ${r.body}`);
}

export default function () {
  put(`/q/schemas/${TENANT}`, schema());
  put(`/q/views/${VIEW}`, {
    id: VIEW, tenant: TENANT, selector: TYPES,
    rules: [{ types: TYPES, attributes: ["name", "level", "category", "lane", "sentAt", "pm25", "startDate", "endDate", "location"] }],
    operations: ["query_entities", "retrieve_entity", "retrieve_entity_types", "temporal_query", "temporal_retrieve"],
  });
  for (let n = 0; n < HISTORY_ENTITIES; n++) {
    const pm25 = [];
    for (let s = 0; s < 120; s++) {
      pm25.push({ type: "Property", value: 5 + ((n * 7 + s * 13) % 60), observedAt: new Date(HISTORY_START + s * STEP_MS).toISOString() });
    }
    const r = http.post(`${ROOT}/ngsi-ld/v1/temporal/entities`,
      JSON.stringify({ id: `urn:ngsi-ld:AirQualityObserved:load:${n}`, type: "AirQualityObserved", pm25 }), { headers: WRITE });
    // k6 exits 0 when an iteration throws: run.sh greps the log for this line
    if (r.status !== 201 && r.status !== 204) throw new Error(`history ${n}: ${r.status} ${r.body}`);
  }
}
