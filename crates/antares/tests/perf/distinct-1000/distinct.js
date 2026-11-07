// 1 000 distinct requests a second for 10 minutes (T-5062): an open model (constant arrival
// rate), request number u of the run from queries.js, so no request repeats and no answer
// cache can serve one. run.sh seeds the broker with dev/perf/k6-surfaces.js first.
//
//   k6 run -e BASE_URL=http://127.0.0.1:9471 tests/perf/distinct-1000/distinct.js
//   k6 run --local-ips=127.0.0.10-127.0.0.73 ...   # 64 source addresses (run.sh does this)
//
// Env: BASE_URL, RATE (1000), DURATION (10m), VUS (RATE / 2), MAX_VUS (RATE * 2), ENTITIES
//      (581000), EVENTS (5000), HISTORY_ENTITIES (500), VIEW (load), SUBJECT and ROLES (the
//      subject headers the gateway would send to a policy build; unset sends none), ENDPOINT
//      (a slug: every request goes to BASE_URL/api/endpoint/<slug>/…, the Context Gateway),
//      P95_MS (300).
// Fails (exit 99) when p95 >= P95_MS, errors >= 0.1 % or more than 0.1 % of the arrivals were
// dropped for want of a free VU.

import http from "k6/http";
import exec from "k6/execution";
import { Rate } from "k6/metrics";
import { request, MIX } from "./queries.js";

const BASE = __ENV.BASE_URL || "http://127.0.0.1:9471";
const RATE = Number(__ENV.RATE || 1000);
const DURATION = __ENV.DURATION || "10m";
const SECONDS = Number(DURATION.replace(/m$/, "")) * (DURATION.endsWith("m") ? 60 : 1) || 600;
const O = {
  entities: Number(__ENV.ENTITIES || 581000),
  events: Number(__ENV.EVENTS || 5000),
  historyEntities: Number(__ENV.HISTORY_ENTITIES || 500),
  view: __ENV.VIEW || "load",
  endpoint: __ENV.ENDPOINT,
};
const HEADERS = { "Content-Type": "application/json", Accept: "application/json" };
if (__ENV.SUBJECT) HEADERS["x-jc-subject"] = __ENV.SUBJECT;
if (__ENV.ROLES) HEADERS["x-jc-roles"] = __ENV.ROLES;

const thresholds = {
  http_req_failed: ["rate<0.001"],
  http_req_duration: [`p(95)<${Number(__ENV.P95_MS || 300)}`],
  dropped_iterations: [`count<${Math.ceil(RATE * SECONDS * 0.001)}`],
  answered: ["rate>0.999"],
};
// one latency line per shape in the summary
for (const [shape] of MIX) thresholds[`http_req_duration{shape:${shape}}`] = ["p(95)>=0"];

export const options = {
  discardResponseBodies: false,
  summaryTrendStats: ["avg", "p(50)", "p(95)", "p(99)", "max"],
  scenarios: {
    distinct: {
      executor: "constant-arrival-rate",
      rate: RATE, timeUnit: "1s", duration: DURATION,
      preAllocatedVUs: Number(__ENV.VUS || RATE / 2), maxVUs: Number(__ENV.MAX_VUS || RATE * 2),
    },
  },
  thresholds,
};

// MCP streamable HTTP wants both types named and the protocol version beside the request's
// _meta; the CSV page is asked by f=csv and its type
const ACCEPT = {
  csv: { Accept: "text/csv" },
  mcp: { Accept: "application/json, text/event-stream", "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "tools/call", "Mcp-Name": "query_entities" },
};
const answered = new Rate("answered");
let warned = 0; // per VU: the first refusals name their request, the rest only count

export default function () {
  const u = exec.scenario.iterationInTest;
  const r = request(u, O);
  const headers = { ...HEADERS, ...ACCEPT[r.shape] };
  const res = http.request(r.method, `${BASE}${r.path}`, r.body || null, {
    headers, tags: { shape: r.shape, name: r.shape }, timeout: "10s",
  });
  // an MCP tool error is a 200 with isError: it counts as a failed answer
  let ok = res.status === 200;
  if (ok && r.shape === "mcp") {
    try {
      const body = res.json();
      ok = !body.error && !(body.result && body.result.isError);
    } catch (_) {
      ok = false;
    }
  }
  if (!ok && warned++ < 2) console.warn(`${r.shape} ${res.status} ${r.path} ${String(res.body).slice(0, 200)}`);
  answered.add(ok, { shape: r.shape });
}
