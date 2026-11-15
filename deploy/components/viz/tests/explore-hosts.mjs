// The hosts of the explorer journeys (T-5059, T-5082, T-5129), shared by explore, embed and wordpress
// specs: Playwright refuses a spec importing another spec.
import { readFileSync, mkdirSync } from "node:fs";
import { gunzipSync } from "node:zlib";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { mvt } from "./mvt.mjs";

export const here = path.dirname(fileURLToPath(import.meta.url));
const chart = path.join(here, "../charts/renderer");
const vendor = path.join(here, "../../../images/viz/vendor");
const explorer = path.join(here, "../../../images/viz/explorer");
export const shots = path.join(here, "test-results/explore");
mkdirSync(shots, { recursive: true });
export const DOMAIN = "dev.joinedcontext.com";
export const VIZ = `viz.${DOMAIN}`;
export const CATALOGUE = `opendata.${DOMAIN}`;
export const API = `api.${DOMAIN}`;

const conf = readFileSync(path.join(chart, "files/nginx.conf"), "utf8").replaceAll("__HOST__", VIZ).replaceAll("__DOMAIN__", DOMAIN);
const csp = (location) => /Content-Security-Policy "([^"]+)"/.exec(conf.slice(conf.indexOf(`location = ${location} {`)))[1];

// keyValues records per data set; the normalized form is derived from them.
const bike = (i, extra = {}) => ({
  id: `urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:${String(i).padStart(3, "0")}`, type: "BikeHireDockingStation",
  name: { languageMap: { fi: `Asema ${i}`, sk: `Stanica ${i}`, en: `Station ${i}` } }, status: "working", availableBikeNumber: 20 - i,
  dateModified: "2026-10-21T19:35:26Z", source: "https://www.hsl.fi/en/citybikes",
  location: { type: "Point", coordinates: [24.95 + i / 100, 60.155 + i / 200] }, ...extra
});
const square = (x, y) => ({ type: "Polygon", coordinates: [[[x, y], [x + 0.05, y], [x + 0.05, y + 0.03], [x, y + 0.03], [x, y]]] });
// The model as Model Tools publishes it: class attributes with ranks, labels as JSON text.
const label = (sk, en) => `{tag: label, value: '${JSON.stringify({ sk, en })}'}`;
const EVENT_MODEL = `id: https://example.test/events
name: events
classes:
  Event:
    annotations: {view_order_by: startDate, view_upcoming: startDate}
    attributes:
      name: {rank: 1, annotations: {label: ${label("Názov", "Name")}}}
      startDate: {rank: 2, annotations: {label: ${label("Začiatok", "Start")}}}
      location: {rank: 4, annotations: {ngsi_ld_kind: {tag: ngsi_ld_kind, value: GeoProperty}}}
      venue: {rank: 5, annotations: {label: ${label("Miesto", "Venue")}}}
      address: {rank: 6, annotations: {label: ${label("Adresa", "Address")}}}
      organizer: {rank: 8, annotations: {label: ${label("Organizátor", "Organizer")}}}
      url: {rank: 9, annotations: {label: ${label("Web", "Web")}}}
      dataProvider: {range: string}
`;
const kpi = (i) => ({
  id: `urn:ngsi-ld:KeyPerformanceIndicator:susr:pl2016rs-${i}`, type: "KeyPerformanceIndicator", name: "Stavy hospodárskych zvierat k 31.12.",
  kpiValue: 1330091 + i, refArea: "SK0", refPeriod: String(1970 + (i % 50)), dataSet: "pl2016rs"
});
export const DATA = {
  point: { type: "BikeHireDockingStation", rows: [1, 2, 3].map((i) => bike(i)) },
  polygon: { type: "AdministrativeArea", rows: [0, 1].map((i) => ({ id: `urn:ngsi-ld:AdministrativeArea:bb:${i}`, type: "AdministrativeArea", name: `Mestská časť ${i + 1}`, population: 12000 + i, location: square(19.1 + i * 0.06, 48.72) })) },
  nogeo: { type: "KeyPerformanceIndicator", rows: [1, 2, 3, 4].map(kpi) },
  zero: { type: "KeyPerformanceIndicator", rows: [], sample: [kpi(1)] }, // the schema lists attributes, the broker holds none
  one: { type: "BikeHireDockingStation", rows: [bike(7)] },
  // T-5189: more than the map's 1000 in view, so the map draws the type's vector tiles.
  tiled: { type: "BikeHireDockingStation", count: 50000, tiles: true, rows: [1, 2, 3].map((i) => bike(i)) },
  many: { type: "KeyPerformanceIndicator", count: 12345, rows: null },
  long: { type: "Event", rows: [{ id: "urn:ngsi-ld:Event:bbsk:" + "x".repeat(80), type: "Event", name: "Festival " + "veľmi dlhý názov podujatia ".repeat(6),
    description: "Opis <img src=x onerror=alert(1)> ".repeat(20), url: "https://www.bbsk.sk/podujatia/" + "a-b-c-".repeat(30), startDate: "2026-11-01T18:00:00Z" }] },
  multi: { type: "WeatherObserved", rows: [{ id: "urn:ngsi-ld:WeatherObserved:shmu:11894", type: "WeatherObserved", name: { languageMap: { sk: "Stanica Donovaly", en: "Donovaly station" } },
    temperature: [11.5, 12.1], address: { addressLocality: "Donovaly", postalCode: "97639" }, refDevice: "urn:ngsi-ld:Device:1" }] },
  notypes: { types: [] },
  // T-5192: the bbsk-podujatia shape, 30 events with a published model; one name and one venue of
  // 500 characters, a third without a start, and the model's ranks and labels.
  events: { type: "Event", ownQuery: true, model: EVENT_MODEL, rows: Array.from({ length: 30 }, (_, i) => ({
    id: `urn:ngsi-ld:Event:bbsk:${i}`, type: "Event",
    name: i === 0 ? "Koncert " + "dlhý názov ".repeat(45) : `Podujatie ${i + 1}`,
    ...(i % 3 === 2 ? {} : { startDate: `2026-11-${String((i % 28) + 1).padStart(2, "0")}T18:00:00+01:00` }),
    venue: i === 1 ? "Sála " + "veľká ".repeat(80) : "Kino Baník",
    address: "Námestie SNP 14, Banská Bystrica",
    // T-5134: the model's organizer object; every third one names no web site and reads as text.
    organizer: i % 3 === 1 ? { name: "Mestské kultúrne stredisko" } : { name: "Mestské kultúrne stredisko", url: "https://www.mksbb.sk/" },
    url: `https://www.bbsk.sk/podujatia/${i}`,
    dataProvider: "Banská Bystrica, bbsk.sk (licencia neuvedená)",
    location: { type: "Point", coordinates: [19.14 + i / 200, 48.73 + i / 400] }
  })) }
};
// T-5233: 12 stations, a model with two GeoProperties. 0-7 have a location, 0-9 an observation
// space 0.8° east of it, 10-11 neither; `open` (boolean) colours the map. The mock honours georel
// within, geoproperty, geometryProperty, limit=0 and a q of `!attr` and `open==true` terms.
const STATION_MODEL = `id: https://example.test/stations
name: stations
classes:
  Station:
    annotations: {label: ${label("Stanica", "Station")}}
    attributes:
      name: {rank: 1, annotations: {label: ${label("Názov", "Name")}}}
      location: {rank: 2, annotations: {label: ${label("Poloha", "Location")}, ngsi_ld_kind: {tag: ngsi_ld_kind, value: GeoProperty}}}
      observationSpace: {rank: 3, annotations: {label: ${label("Priestor pozorovania", "Observation space")}, ngsi_ld_kind: {tag: ngsi_ld_kind, value: GeoProperty}}}
      open: {rank: 4, annotations: {label: ${label("Otvorená", "Open")}}}
      url: {rank: 5, annotations: {label: ${label("Web", "Web")}}}
`;
const station = (i) => ({
  id: `urn:ngsi-ld:Station:zsr:${i}`, type: "Station", name: `Stanica ${i + 1}`, open: i % 2 === 0, url: `https://www.zsr.sk/stanica/${i}`,
  ...(i < 8 ? { location: { type: "Point", coordinates: [18.7 + i * 0.05, 49.2 + i * 0.02] } } : {}),
  ...(i < 10 ? { observationSpace: { type: "Point", coordinates: [19.5 + i * 0.05, 49.2 + i * 0.02] } } : {})
});

// T-5196, DM-134: the same events, presented by the model alone: a default filter, 10 rows a page.
DATA.presented = {
  ...DATA.events,
  model: EVENT_MODEL.replace("{view_order_by: startDate, view_upcoming: startDate}", "{view_order_by: startDate, view_filter: startDate>=today, view_page_length: 10}")
};
// T-5197, API/11 EP-164: the presented events, with the Endpoint's table start above the model's.
DATA.publishedtable = { ...DATA.presented, widget: { map: {}, table: { Event: { filter: "startDate>=today+7d", pageLength: 20, columns: ["name", "startDate"] } } } };
DATA.stations = { type: "Station", geo: ["location", "observationSpace"], model: STATION_MODEL, rows: Array.from({ length: 12 }, (_, i) => station(i)) };
// T-5239: the same stations, with the publisher's map defaults the Portal hands over at widget.json.
DATA.publishedmap = { ...DATA.stations, widget: { map: { Station: { colour: "open", popup: ["url"], cluster: false } } } };
export const SLUGS = Object.fromEntries(Object.keys(DATA).map((k) => [k, (k + "slug").padEnd(32, "0").slice(0, 32)]));
const FAIL = { forbidden: 403, missing: 404, gateway: 502, slow: 0 };
for (const k of Object.keys(FAIL)) SLUGS[k] = (k + "fail").padEnd(32, "0").slice(0, 32);
const key = (slug) => Object.keys(SLUGS).find((k) => SLUGS[k] === slug);

const attrs = (rows) => [...new Set(rows.flatMap((r) => Object.keys(r).filter((a) => a !== "id" && a !== "type")))];
const normal = (r) => Object.fromEntries(Object.entries(r).map(([k, v]) => {
  if (k === "id" || k === "type") return [k, v];
  if (k === "location" || k === "observationSpace") return [k, { type: "GeoProperty", value: v }];
  if (v && v.languageMap) return [k, { type: "LanguageProperty", languageMap: v.languageMap }];
  if (k.startsWith("ref")) return [k, { type: "Relationship", object: v }];
  if (Array.isArray(v)) return [k, v.map((x, i) => ({ type: "Property", value: x, datasetId: `urn:ds:${i}` }))];
  return [k, { type: "Property", value: v }];
}));
const localized = (r, lang) => Object.fromEntries(Object.entries(r).map(([k, v]) => [k, v && v.languageMap ? v.languageMap[lang] || Object.values(v.languageMap)[0] : v]));
const manyRows = (offset, limit) => Array.from({ length: Math.max(0, Math.min(limit, 12345 - offset)) }, (_, i) => kpi(offset + i + 1));

// The Endpoint's JSON Schema as the gateway serves it: one definition, a type and a description per attribute.
const kindOf = (v) => (typeof v === "number" ? { type: "number" } : typeof v === "boolean" ? { type: "boolean" } : typeof v === "string" && /^\d{4}-\d{2}-\d{2}T/.test(v) ? { type: "string", format: "date-time" }
  : typeof v === "string" && /^https:\/\//.test(v) ? { type: "string", format: "uri" } : { type: "string" });
const schemaOf = (d) => {
  const sample = d.sample || d.rows || manyRows(0, 5);
  const properties = Object.fromEntries(attrs(sample).map((a) => [a, { ...kindOf(sample.find((r) => r[a] !== undefined)[a]), description: `The ${a} of the ${d.type}` }]));
  return { $defs: { [d.type]: { type: "object", properties } } };
};
const problem = (status) => ({ type: "about:blank", title: "refused", status, detail: { 403: "no policy grants anonymous reads on this endpoint", 404: "the endpoint does not exist", 502: "the broker did not answer" }[status] });
const valueOf = (r, a) => (r[a] && r[a].languageMap ? Object.values(r[a].languageMap)[0] : r[a]);
const ordered = (rows, orderBy) => {
  if (!orderBy) return rows;
  const [a, dir] = orderBy.split(";");
  const f = dir === "desc" ? -1 : 1;
  return [...rows].sort((x, y) => (valueOf(x, a) === undefined) - (valueOf(y, a) === undefined) || (typeof valueOf(x, a) === "number" ? f * (valueOf(x, a) - valueOf(y, a)) : f * String(valueOf(x, a)).localeCompare(String(valueOf(y, a)))));
};

export async function serve(context) {
  const requests = [];
  await context.route("**/*", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    requests.push({ url, headers: await request.allHeaders() });
    const cors = { "Access-Control-Allow-Origin": `https://${VIZ}`, "Access-Control-Expose-Headers": "NGSILD-Results-Count" };
    if (url.hostname === VIZ) {
      // /embed.html is /explore.html under its own CSP (nginx try_files, T-5129).
      const page = ["/explore.html", "/embed.html"].includes(url.pathname);
      const headers = { "Content-Security-Policy": page ? csp(url.pathname) : "default-src 'none'; frame-ancestors 'none'" };
      try {
        const at = url.pathname.startsWith("/vendor/") ? path.join(vendor, url.pathname.slice("/vendor/".length))
          : url.pathname.startsWith("/explorer/") ? path.join(explorer, url.pathname.slice("/explorer/".length))
            : path.join(chart, "files/site", url.pathname === "/embed.html" ? "/explore.html" : url.pathname);
        const body = at.endsWith(".mjs") ? gunzipSync(readFileSync(at + ".gz")) : readFileSync(at);
        const type = at.endsWith(".html") ? "text/html" : at.endsWith(".css") ? "text/css" : "text/javascript";
        return route.fulfill({ status: 200, contentType: type, headers, body });
      } catch {
        return route.fulfill({ status: 404, body: "" });
      }
    }
    if (url.hostname === CATALOGUE) {
      // The dataset page as jc_explorer.html renders it, with jc-viz.js sizing the frame.
      const [, , name] = url.pathname.split("/");
      const q = url.searchParams;
      return route.fulfill({
        status: 200, contentType: "text/html",
        body: `<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1"><title>dataset</title>
          <style>body{margin:0;font:15px system-ui;background:${q.get("theme") === "dark" ? "#0b1220" : "#fff"}}</style>
          <iframe id="explore" class="jc-viz-frame jc-explorer-frame" data-jc-viz="https://${VIZ}" title="Live data" style="width:100%;height:600px;border:0"
            sandbox="allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox" referrerpolicy="no-referrer"
            src="https://${VIZ}/explore.html#${SLUGS[name]}&inline&lang=${q.get("lang") || "en"}${q.get("theme") ? "&theme=" + q.get("theme") : ""}${q.get("extra") || ""}"></iframe>
          <script>${readFileSync(path.join(here, "../../../charts/ckan/files/jc_theme/public__jc-viz.js"), "utf8")}</script>`
      });
    }
    if (url.hostname === `tiles.${DOMAIN}` && url.pathname.startsWith("/styles/light-")) {
      return route.fulfill({ status: 200, contentType: "application/json", headers: cors,
        body: JSON.stringify({ version: 8, sources: {}, layers: [{ id: "paper", type: "background", paint: { "background-color": "#e8ecef" } }] }) });
    }
    if (url.hostname === API) {
      const t = /^\/api\/endpoint\/([a-z0-9]+)\/ogc\/features\/collections\/([A-Za-z]+)\/tiles\/WebMercatorQuad(?:\/(\d+)\/(\d+)\/(\d+))?$/.exec(url.pathname);
      const one = /^\/api\/endpoint\/([a-z0-9]+)\/ngsi-ld\/v1\/entities\/([^/]+)$/.exec(url.pathname);
      const own = t ? DATA[key(t[1])] : one ? DATA[key(one[1])] : null;
      if (t && (!own || !own.tiles || own.type !== t[2])) return route.fulfill({ status: 404, headers: cors, body: "{}" });
      if (t && !t[3]) {
        return route.fulfill({ status: 200, contentType: "application/json", headers: cors, body: JSON.stringify({ dataType: "vector", links: [{ rel: "item",
          type: "application/vnd.mapbox-vector-tile", templated: true, href: `https://${API}${url.pathname}/{tileMatrix}/{tileRow}/{tileCol}` }] }) });
      }
      if (t) {
        const body = mvt(own.type, own.rows.map((r) => ({ id: r.id, geometry: r.location, properties: {} })), +t[3], +t[5], +t[4]);
        return route.fulfill(body ? { status: 200, contentType: "application/vnd.mapbox-vector-tile", headers: cors, body } : { status: 204, headers: cors });
      }
      if (one) {
        const r = own && (own.rows || []).find((x) => x.id === decodeURIComponent(one[2]));
        return r ? route.fulfill({ status: 200, contentType: "application/json", headers: cors, body: JSON.stringify(localized(r, url.searchParams.get("lang"))) })
          : route.fulfill({ status: 404, headers: cors, body: "{}" });
      }
      const typeOne = /^\/api\/endpoint\/([a-z0-9]+)\/ngsi-ld\/v1\/types\/([A-Za-z]+)$/.exec(url.pathname);
      if (typeOne) {
        const d = DATA[key(typeOne[1])];
        if (!d || !d.geo || d.type !== typeOne[2]) return route.fulfill({ status: 404, headers: cors, body: "{}" });
        const details = attrs(d.rows).map((a) => ({ id: a, type: "Attribute", attributeName: a, attributeTypes: [d.geo.includes(a) ? "GeoProperty" : "Property"] }));
        return route.fulfill({ status: 200, contentType: "application/json", headers: cors, body: JSON.stringify({ id: d.type, type: "EntityTypeInfo", typeName: d.type, entityCount: d.rows.length, attributeDetails: details }) });
      }
      const w = /^\/api\/endpoint\/([a-z0-9]+)\/widget\.json$/.exec(url.pathname);
      if (w) {
        const d = DATA[key(w[1])];
        return route.fulfill(d ? { status: 200, contentType: "application/json", headers: cors, body: JSON.stringify(d.widget || { map: {} }) } : { status: 404, headers: cors, body: "{}" });
      }
      const m = /^\/api\/endpoint\/([a-z0-9]+)\/(ngsi-ld\/v1\/types|ngsi-ld\/v1\/entities|schema\/index\.json|schema\/v1\/json-schema|schema\/v1\/model\.linkml\.yaml)$/.exec(url.pathname);
      const name = m && key(m[1]);
      if (!name) return route.fulfill({ status: 404, headers: cors, body: "{}" });
      if (name in FAIL) {
        if (!FAIL[name]) return; // never answers
        return route.fulfill({ status: FAIL[name], headers: cors, contentType: "application/problem+json", body: JSON.stringify(problem(FAIL[name])) });
      }
      const d = DATA[name];
      const json = (body, count) => route.fulfill({ status: 200, contentType: request.headers().accept === "application/geo+json" ? "application/geo+json" : "application/json",
        headers: { ...cors, ...(count === undefined ? {} : { "NGSILD-Results-Count": String(count) }) }, body: JSON.stringify(body) });
      if (m[2] === "schema/index.json") return json({ models: d.types ? [] : [{ name: d.type, version: 1 }] });
      if (m[2] === "schema/v1/json-schema") return json(d.types ? { $defs: {} } : schemaOf(d));
      if (m[2] === "schema/v1/model.linkml.yaml") {
        return d.model ? route.fulfill({ status: 200, contentType: "text/yaml", headers: cors, body: d.model }) : route.fulfill({ status: 404, headers: cors, body: "" });
      }
      if (m[2].endsWith("types")) {
        if (d.types) return json(d.types);
        const sample = d.sample || d.rows || manyRows(0, 5);
        return json([{ id: `https://uri.etsi.org/ngsi-ld/default-context/${d.type}`, type: "EntityType", typeName: d.type, attributeNames: attrs(sample) }]);
      }
      const p = url.searchParams;
      const offset = +(p.get("offset") || 0);
      const limit = +(p.get("limit") || 20);
      let rows = d.rows || manyRows(offset, limit);
      let count = d.count ?? rows.length;
      if (d.geo) {
        const gp = p.get("geoproperty") || "location";
        for (const term of (p.get("q") || "").replace(/[()]/g, "").split(";").filter(Boolean)) {
          const not = /^!(\w+)$/.exec(term), is = /^open==(true|false)$/.exec(term);
          if (not) rows = rows.filter((r) => r[not[1]] === undefined);
          else if (is) rows = rows.filter((r) => r.open === (is[1] === "true"));
        }
        if (p.get("georel") === "within") {
          const ring = JSON.parse(p.get("coordinates"))[0];
          const [w, s0, e, n] = [ring[0][0], ring[0][1], ring[2][0], ring[2][1]];
          rows = rows.filter((r) => r[gp] && r[gp].coordinates[0] >= w && r[gp].coordinates[0] <= e && r[gp].coordinates[1] >= s0 && r[gp].coordinates[1] <= n);
        }
        count = rows.length;
        if (limit === 0) rows = [];
      }
      if (p.get("q") && !d.ownQuery && !d.geo) { rows = rows.slice(0, 1); count = rows.length; }
      rows = ordered(rows, p.get("orderBy"));
      if (d.rows) rows = rows.slice(offset, offset + limit);
      if (request.headers().accept === "application/geo+json") {
        const g = p.get("geometryProperty") || "location";
        return json({ type: "FeatureCollection", features: rows.filter((r) => r[g]).map((r) => ({ id: r.id, type: "Feature", geometry: r[g], properties: localized(r, p.get("lang")) })) }, count);
      }
      return json(p.get("format") === "keyValues" ? rows.map((r) => localized(r, p.get("lang"))) : rows.map(normal), count);
    }
    return route.abort();
  });
  return requests;
}

// T-5129: the catalogue datasets an embed names for its source line.
export const PACKAGES = {
  "bike-stations": { name: "bike-stations", title: "Stanice zdieľaných bicyklov", license_title: "Creative Commons Attribution 4.0",
    license_url: "https://creativecommons.org/licenses/by/4.0/", organization: { title: "Mesto Banská Bystrica" },
    extras: [{ key: "endpoint", value: `https://${API}/api/endpoint/${SLUGS.point}/` }] },
  "other-endpoint": { name: "other-endpoint", title: "Iný dataset", license_title: "Proprietary", license_url: "", organization: { title: "Niekto iný" },
    extras: [{ key: "endpoint", value: `https://${API}/api/endpoint/${SLUGS.polygon}/` }] }
};

// The catalogue's action API (CORS for viz alone, as CKAN__CORS__ORIGIN_WHITELIST) and its dataset
// page with the explorer frame jc_explorer.html renders, the dataset named in the hash.
export async function catalogue(context) {
  await context.route((url) => url.hostname === CATALOGUE, (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === "/api/3/action/package_show") {
      const pkg = PACKAGES[url.searchParams.get("id")];
      return route.fulfill({ status: pkg ? 200 : 404, contentType: "application/json", headers: { "Access-Control-Allow-Origin": `https://${VIZ}` },
        body: JSON.stringify(pkg ? { success: true, result: pkg } : { success: false, error: { message: "Not found" } }) });
    }
    const lang = url.searchParams.get("lang") || "en";
    return route.fulfill({ status: 200, contentType: "text/html", body: `<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1"><title>dataset</title>
      <style>body{margin:0;font:15px system-ui}</style>
      <iframe id="explore" class="jc-viz-frame jc-explorer-frame" data-jc-viz="https://${VIZ}" title="Live data" style="width:100%;height:600px;border:0" allow="clipboard-write"
        sandbox="allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox" referrerpolicy="no-referrer"
        src="https://${VIZ}/explore.html#${SLUGS.point}&inline&lang=${lang}&dataset=bike-stations${url.searchParams.get("extra") || ""}"></iframe>
      <script>${readFileSync(path.join(here, "../../../charts/ckan/files/jc_theme/public__jc-viz.js"), "utf8")}</script>` });
  });
}
