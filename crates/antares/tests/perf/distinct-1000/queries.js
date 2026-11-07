// The query generator of the distinct-1000 bench (T-5062): request number u of the run becomes
// one request, and no two numbers below 1 000 000 give the same request. Each shape puts u into
// a parameter (an entity number, the last digits of a coordinate or a threshold), so a cache
// keyed on the request never hits and the store answers every one.
// Plain ES module without k6 imports: distinct.js runs it under k6, `node queries.js` runs the
// self-test below.
//
// The data is dev/perf/k6-surfaces.js's seed: ten types, every entity a Point in the box
// lon 19..20, lat 48.5 + 0.001 per 1 000 entities of a type (up to 49), `level` 0..99, `category` c0..c7, a pm25 history on the first
// HISTORY_ENTITIES AirQualityObserved. Synthetic only: no real place, person or device.

export const TYPES = [
  "AirQualityObserved", "Vehicle", "Building", "ParkingSpot", "Streetlight",
  "WeatherObserved", "WaterQualityObserved", "TrafficFlowObserved", "Device", "Event",
];
export const SPAN = 1000000; // distinct requests per run: 10 min at 1 000/s is 600 000

// [shape, weight in percent]: the mix of the 1 000 req/s
export const MIX = [
  ["retrieve", 18], ["q2", 14], ["q3", 10], ["near", 10], ["within", 8], ["temporal", 8],
  ["odata", 10], ["ogc-bbox", 10], ["csv", 6], ["mcp", 6],
];

// mulberry32: a stateless hash of u, so a request depends on u alone (any VU, any order)
function hash(u, salt) {
  let t = (u * 2654435761 + salt * 40503 + 0x6d2b79f5) >>> 0;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}

export function shapeOf(u) {
  let p = (u * 7919) % 100; // a fixed walk over the percent cells: the mix holds over any 100 numbers
  for (const [shape, w] of MIX) {
    if (p < w) return shape;
    p -= w;
  }
  throw new Error("MIX does not sum to 100");
}

// a value that carries u in its last six decimals: distinct for every u below SPAN
const tag = (base, u) => (Math.floor(base) + (u % SPAN) / SPAN).toFixed(6);
const enc = encodeURIComponent;

// options: { entities, events, historyEntities, view, endpoint } as the seed was run; `endpoint` (a slug)
// asks every surface through the Context Gateway's /api/endpoint/{slug}/… instead of the broker
export function request(u, o) {
  const shape = shapeOf(u);
  const type = TYPES[Math.floor(hash(u, 1) * (TYPES.length - 1))]; // not Event: its count is small
  const per = Math.floor((o.entities - o.events) / (TYPES.length - 1));
  // a thousandth of the box by the hash, u in the nine decimals below it: every point its own
  const lon = Number((19 + Math.floor(hash(u, 9) * 950) / 1000 + (u % SPAN) / 1e9).toFixed(9));
  // the seed fills lat 48.5 + a thousandth per 1 000 entities of a type: stay on the filled rows
  const lat = 48.5 + hash(u, 2) * Math.min(0.45, Math.ceil(per / 1000) / 1000);
  const lo = Math.floor(hash(u, 3) * 80);
  const cat = `c${Math.floor(hash(u, 4) * 8)}`;
  const view = o.endpoint ? `/api/endpoint/${o.endpoint}` : `/x/views/${o.view}`;
  const base = o.endpoint ? `${view}/ngsi-ld/v1` : "/ngsi-ld/v1";
  switch (shape) {
    case "retrieve": {
      // u walks every entity once (type by u, number by u / 9); the next walk asks another
      // attrs subset of the same ids, so a request repeats only after 15 walks
      const g = u % (9 * per);
      const id = `urn:ngsi-ld:${TYPES[g % 9]}:load:${Math.floor(g / 9)}`;
      const walk = 1 + (Math.floor(u / (9 * per)) % 15);
      const attrs = ["name", "level", "category", "location"].filter((_, i) => (walk >> i) & 1);
      return { shape, method: "GET", path: `${base}/entities/${enc(id)}?attrs=${attrs.join(",")}` };
    }
    case "q2":
      return { shape, method: "GET", path: `${base}/entities?type=${type}&q=${enc(`level>=${tag(lo, u)};level<${lo + 10}`)}&limit=20` };
    case "q3":
      return { shape, method: "GET", path: `${base}/entities?type=${type}&q=${enc(`level>=${tag(lo, u)};level<${lo + 20};category=="${cat}"`)}&limit=20` };
    case "near": {
      const d = 200 + Math.floor(hash(u, 5) * 3000);
      return { shape, method: "GET", path: `${base}/entities?type=${type}&georel=${enc(`near;maxDistance==${d}`)}&geometry=Point&coordinates=${enc(`[${lon},${lat.toFixed(6)}]`)}&limit=20` };
    }
    case "within": {
      const w = 0.01 + hash(u, 6) * 0.04;
      const x1 = Number((lon + w).toFixed(9)), y1 = Number((lat + w / 2).toFixed(6));
      const box = [[[lon, lat], [x1, lat], [x1, y1], [lon, y1], [lon, lat]]];
      return { shape, method: "GET", path: `${base}/entities?type=${type}&georel=within&geometry=Polygon&coordinates=${enc(JSON.stringify(box))}&limit=20` };
    }
    case "temporal": {
      // a window of 1..7 days starting at a second carried by u: the history is 2026-09-01 + 30 days
      const from = Date.parse("2026-09-01T00:00:00Z") + (u % SPAN) * 1000;
      const to = from + (1 + Math.floor(hash(u, 7) * 7)) * 86400000;
      const id = `urn:ngsi-ld:AirQualityObserved:load:${u % o.historyEntities}`;
      return { shape, method: "GET", path: `${base}/temporal/entities/${enc(id)}?attrs=pm25&timerel=between&timeAt=${new Date(from).toISOString()}&endTimeAt=${new Date(to).toISOString()}` };
    }
    case "odata":
      return { shape, method: "GET", path: `${view}/odata/v4/${type}?$filter=${enc(`level ge ${tag(lo, u)} and level lt ${lo + 10} and category eq '${cat}'`)}&$top=20` };
    case "ogc-bbox":
    case "csv": {
      const w = 0.02 + hash(u, 8) * 0.05;
      const bbox = `${lon},${lat.toFixed(6)},${(lon + w).toFixed(9)},${(lat + w / 2).toFixed(6)}`;
      const f = shape === "csv" ? "&f=csv" : "";
      return { shape, method: "GET", path: `${view}/ogc/features/collections/${type}/items?bbox=${bbox}&limit=50${f}` };
    }
    case "mcp":
      return {
        shape, method: "POST", path: `${view}/mcp`,
        body: JSON.stringify({
          jsonrpc: "2.0", id: u, method: "tools/call",
          params: {
            name: "query_entities",
            arguments: { type: [type], q: `level>=${tag(lo, u)};category=="${cat}"`, limit: 20 },
            _meta: {
              "io.modelcontextprotocol/protocolVersion": "2026-07-28",
              "io.modelcontextprotocol/clientInfo": { name: "distinct-1000", version: "1" },
              "io.modelcontextprotocol/clientCapabilities": {},
            },
          },
        }),
      };
    default:
      throw new Error(`unknown shape ${shape}`);
  }
}

// node tests/perf/distinct-1000/queries.js: every request of the 600 000 of a 10 min run is distinct and
// the mix holds
function selfTest() {
  const o = { entities: 581000, events: 5000, historyEntities: 500, view: "load" };
  const seen = new Set();
  const counts = {};
  const N = 600000;
  for (let u = 0; u < N; u++) {
    const r = request(u, o);
    const key = `${r.method} ${r.path} ${r.body || ""}`;
    if (seen.has(key)) throw new Error(`request ${u} repeats: ${key}`);
    seen.add(key);
    counts[r.shape] = (counts[r.shape] || 0) + 1;
  }
  // through the gateway every surface sits under the Endpoint and the request is the same otherwise
  for (let u = 0; u < 1000; u++) {
    const direct = request(u, o), gw = request(u, { ...o, endpoint: "bench" });
    const want = direct.path.replace(/^\/x\/views\/load/, "/api/endpoint/bench").replace(/^\/ngsi-ld\/v1/, "/api/endpoint/bench/ngsi-ld/v1");
    if (gw.path !== want || gw.body !== direct.body) throw new Error(`request ${u} through the gateway: ${gw.path}, want ${want}`);
  }
  for (const [shape, w] of MIX) {
    const got = counts[shape] / N * 100;
    if (Math.abs(got - w) > 0.5) throw new Error(`${shape}: ${got}% of the mix, want ${w}%`);
  }
  console.log(`queries.js self-test ok: ${N} distinct requests`, counts);
}

if (typeof process !== "undefined" && process.argv && process.argv[1] && process.argv[1].endsWith("queries.js")) selfTest();
