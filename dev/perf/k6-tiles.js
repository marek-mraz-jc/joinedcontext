// A map client panning and zooming over one collection's vector tiles (T-5121):
// each iteration asks the 3 x 3 tiles around a point that drifts through the
// box, at a zoom from ZMIN to ZMAX, as MapLibre does on a pan. SEED > 0 first
// writes that many AirQualityObserved points spread over the box into TENANT
// (batch upserts of 1000), so a 100k-feature run needs no other loader.
//
//   dev/views-rig.sh http://localhost:9090 ci/views/ogc
//   k6 run -e SEED=100000 dev/perf/k6-tiles.js
//
// Env: BROKER_URL (http://localhost:9090), VIEW (ogc-public), COLLECTION
//      (AirQualityObserved), TENANT (ogc), SEED (0), BOX (16.9,47.9,17.4,48.3
//      in CRS84), ZMIN (8), ZMAX (16), VUS (10), DURATION (60s), P95_MS (200).
//      TOKEN, when set, goes as the bearer token of every request.

import http from "k6/http";
import exec from "k6/execution";
import { check } from "k6";

const BROKER = __ENV.BROKER_URL || "http://localhost:9090";
const VIEW = __ENV.VIEW || "ogc-public";
const COLLECTION = __ENV.COLLECTION || "AirQualityObserved";
const TENANT = __ENV.TENANT || "ogc";
const SEED = Number(__ENV.SEED || 0);
const [W, S, E, N] = (__ENV.BOX || "16.9,47.9,17.4,48.3").split(",").map(Number);
const ZMIN = Number(__ENV.ZMIN || 8);
const ZMAX = Number(__ENV.ZMAX || 16);
const AUTH = __ENV.TOKEN ? { Authorization: `Bearer ${__ENV.TOKEN}` } : {};
const TILES = `${BROKER}/x/views/${VIEW}/ogc/features/collections/${COLLECTION}/tiles/WebMercatorQuad`;

export const options = {
  scenarios: {
    pan: {
      executor: "constant-vus",
      vus: Number(__ENV.VUS || 10),
      duration: __ENV.DURATION || "60s",
    },
  },
  thresholds: {
    "http_req_duration{kind:tile}": [`p(95)<${Number(__ENV.P95_MS || 200)}`],
    "checks{kind:tile}": ["rate==1"],
  },
  setupTimeout: "20m",
};

export function setup() {
  const headers = { "Content-Type": "application/json", "NGSILD-Tenant": TENANT, ...AUTH };
  for (let at = 0; at < SEED; at += 1000) {
    const batch = [];
    for (let i = at; i < Math.min(at + 1000, SEED); i++) {
      // a fixed spread, so two runs seed the same points
      const lon = W + ((i * 0.618034) % 1) * (E - W);
      const lat = S + ((i * 0.414214) % 1) * (N - S);
      batch.push({
        id: `urn:ngsi-ld:AirQualityObserved:tiles-${i}`,
        type: "AirQualityObserved",
        temperature: { type: "Property", value: 10 + (i % 20) },
        location: { type: "GeoProperty", value: { type: "Point", coordinates: [lon, lat] } },
      });
    }
    const r = http.post(`${BROKER}/ngsi-ld/v1/entityOperations/upsert`, JSON.stringify(batch), { headers });
    if (r.status !== 201 && r.status !== 204) {
      exec.test.abort(`seed batch at ${at}: ${r.status} ${r.body}`);
    }
  }
}

/** The tile column and row of a position at zoom z (WebMercatorQuad). */
function tileOf(lon, lat, z) {
  const n = 2 ** z;
  const phi = (lat * Math.PI) / 180;
  const col = Math.floor(((lon + 180) / 360) * n);
  const row = Math.floor(((1 - Math.log(Math.tan(phi) + 1 / Math.cos(phi)) / Math.PI) / 2) * n);
  return [col, row];
}

export default function () {
  const i = exec.scenario.iterationInTest;
  const z = ZMIN + (i % (ZMAX - ZMIN + 1));
  const lon = W + ((i * 0.137) % 1) * (E - W);
  const lat = S + ((i * 0.291) % 1) * (N - S);
  const [col, row] = tileOf(lon, lat, z);
  const last = 2 ** z - 1;
  const asks = [];
  for (const dr of [-1, 0, 1]) {
    for (const dc of [-1, 0, 1]) {
      const r = Math.min(last, Math.max(0, row + dr));
      const c = Math.min(last, Math.max(0, col + dc));
      asks.push(["GET", `${TILES}/${z}/${r}/${c}`, null, { headers: AUTH, tags: { kind: "tile" } }]);
    }
  }
  for (const r of http.batch(asks)) {
    check(r, { "tile 200 or 204": (x) => x.status === 200 || x.status === 204 }, { kind: "tile" });
  }
}
