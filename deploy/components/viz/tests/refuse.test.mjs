// T-4641: every rule of the refusal list (ADR-N-053 §2.3, EP-130), as the browser runs it.
//   node --test components/viz/tests/
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const refuse = createRequire(import.meta.url)("../charts/renderer/files/site/refuse.js");
const DOMAIN = "dev.joinedcontext.com";
const VL = "https://vega.github.io/schema/vega-lite/v5.json";
const VEGA = "https://vega.github.io/schema/vega/v5.json";
const rules = (spec) => refuse(spec, DOMAIN).map((e) => `${e.pointer} ${e.rule}`);

test("the docs' examples and a brush pass", () => {
  const line = { $schema: VL, data: { name: "source" }, mark: { type: "line", point: true },
    params: [{ name: "station", select: { type: "point", fields: ["name"] }, bind: "legend" }],
    encoding: { x: { field: "observedAt", type: "temporal" }, opacity: { condition: { param: "station", value: 1 }, value: 0.2 } } };
  const brush = { $schema: VL, data: { name: "source" }, vconcat: [
    { params: [{ name: "time", select: { type: "interval", encodings: ["x"] } }], mark: "line" },
    { transform: [{ filter: { param: "time" } }, { calculate: "datum.temperature * 9 / 5 + 32", as: "f" }], mark: "bar" }] };
  const vega = { $schema: VEGA, data: [{ name: "source" }, { name: "hot", source: "source", transform: [{ type: "filter", expr: "datum.t > 20" }] }], marks: [] };
  const inline = { $schema: "https://vega.github.io/schema/vega-lite/v5.17.0.json", data: { values: [{ a: 1 }] }, mark: "point" };
  const ours = { $schema: VL, data: { name: "source" }, mark: "point", encoding: { href: { value: "https://opendata.dev.joinedcontext.com/dataset/x" } } };
  for (const spec of [line, brush, vega, inline, ours]) assert.deepEqual(rules(spec), []);
});

test("anything but a JSON object of the Vega-Lite 5 or Vega 5 schema", () => {
  for (const spec of [null, [], "x", 1]) assert.match(rules(spec)[0], /a spec is a JSON object/);
  assert.deepEqual(rules({ data: { name: "source" } }), ["/$schema the Vega-Lite 5 or Vega 5 schema"]);
  assert.match(rules({ $schema: "https://vega.github.io/schema/vega-lite/v6.json" })[0], /schema/);
  assert.match(rules({ $schema: "https://evil.example/vega-lite/v5.json" })[0], /schema/);
});

test("over 200 KB", () => {
  const big = { $schema: VL, data: { values: [{ a: "é".repeat(110 * 1024) }] }, mark: "point" };
  assert.ok(rules(big).includes("/ at most 200 KB"));
});

test("a data URL, anywhere, in either grammar", () => {
  assert.deepEqual(rules({ $schema: VL, data: { url: "https://evil.example/x.csv" } }), ["/data data only from the named source or inline values"]);
  assert.deepEqual(rules({ $schema: VL, data: { url: "https://api.dev.joinedcontext.com/x" } }), ["/data data only from the named source or inline values"],
    "not even our own host: rows come from the declared source");
  assert.deepEqual(rules({ $schema: VL, data: { name: "source" }, transform: [{ lookup: "id", from: { data: { url: "//evil.example/x" }, key: "id" } }] }),
    ["/transform/0/from/data data only from the named source or inline values"]);
  assert.deepEqual(rules({ $schema: VEGA, data: [{ name: "x", url: "https://evil.example/x.json" }] }), ["/data data only from the named source or inline values"]);
  assert.deepEqual(rules({ $schema: VL, data: { name: "other" } }), ["/data data only from the named source or inline values"]);
});

test("usermeta and loader options", () => {
  assert.deepEqual(rules({ $schema: VL, usermeta: { embedOptions: { loader: {} } } }), ["/usermeta no usermeta"]);
  assert.deepEqual(rules({ $schema: VL, config: { loader: { baseURL: "https://evil.example/" } } }), ["/config/loader no loader options"]);
});

test("an href or a url that can name a host outside ours", () => {
  const at = (encoding) => rules({ $schema: VL, data: { name: "source" }, mark: "point", encoding });
  assert.deepEqual(at({ href: { field: "link" } }), ["/encoding/href a href only as a literal on our hosts"]);
  assert.deepEqual(at({ href: { value: "https://evil.example/" } }), ["/encoding/href a href only as a literal on our hosts"]);
  assert.deepEqual(at({ href: { value: "https://evildev.joinedcontext.com/" } }), ["/encoding/href a href only as a literal on our hosts"]);
  assert.deepEqual(at({ href: { value: "https://dev.joinedcontext.com.evil.example/" } }), ["/encoding/href a href only as a literal on our hosts"]);
  assert.deepEqual(at({ href: { value: "https://user@dev.joinedcontext.com/" } }), ["/encoding/href a href only as a literal on our hosts"]);
  assert.deepEqual(at({ href: { value: "http://dev.joinedcontext.com/" } }), ["/encoding/href a href only as a literal on our hosts"]);
  // F-DEP2-011: an App's page runs untrusted code, so a link to it is not on our hosts
  assert.deepEqual(at({ href: { value: "https://x.apps.dev.joinedcontext.com/" } }), ["/encoding/href a href only as a literal on our hosts"]);
  assert.ok(at({ href: { value: "javascript:alert(1)" } }).includes("/encoding/href a href only as a literal on our hosts"));
  assert.deepEqual(rules({ $schema: VEGA, marks: [{ type: "symbol", encode: { update: { href: { signal: "'https://evil.example/'" } } } }] }),
    ["/marks/0/encode/update/href a href only as a literal on our hosts"]);
});

test("an image from outside our hosts; a data: image of a raster type is ours", () => {
  const image = (url) => rules({ $schema: VL, data: { name: "source" }, mark: "image", encoding: { url } });
  assert.deepEqual(image({ field: "picture" }), ["/encoding/url a url only as a literal on our hosts"]);
  assert.deepEqual(image({ value: "https://evil.example/pixel.png" }), ["/encoding/url a url only as a literal on our hosts"]);
  assert.deepEqual(image({ value: "data:image/svg+xml;base64,PHN2Zy8+" }), ["/encoding/url a url only as a literal on our hosts"]);
  assert.deepEqual(image({ value: "data:image/png;base64,iVBORw0KGgo=" }), []);
});

test("words an expression has no business with, and a javascript: URL anywhere", () => {
  const calc = (expr) => rules({ $schema: VL, data: { name: "source" }, transform: [{ calculate: expr, as: "x" }], mark: "point" });
  assert.deepEqual(calc("datum.constructor"), ["/transform/0/calculate no __proto__, constructor or prototype"]);
  assert.deepEqual(calc("datum['__proto__']"), ["/transform/0/calculate no __proto__, constructor or prototype"]);
  assert.deepEqual(rules(JSON.parse('{"$schema":"' + VL + '","data":{"name":"source"},"__proto__":{"x":1}}')), ["/__proto__ no __proto__, constructor or prototype"]);
  assert.deepEqual(rules({ $schema: VL, data: { name: "source" }, title: " JavaScript:alert(1)" }), ["/title no javascript: URL"]);
});

// T-4657, ADR-N-053 §2.9: the map spec, GeoJSON and a field list with nothing that runs.
const MAP = "urn:joinedcontext:map:v1";
const station = (coordinates, properties = { name: "Námestie SNP", pm10: 21.5, observedAt: "2026-10-18T15:00:00Z" }) =>
  ({ type: "Feature", geometry: { type: "Point", coordinates }, properties });
const mapSpec = (over = {}) => ({
  $schema: MAP, title: "Kvalita ovzdušia", label: "name", time: "observedAt",
  popup: [{ field: "pm10", label: "PM10", unit: "µg/m³" }],
  data: { type: "FeatureCollection", features: [station([19.1462, 48.7363])] },
  link: "https://opendata.dev.joinedcontext.com/dataset/ovzdusie", ...over
});
const mapRules = (spec) => refuse.map(spec, DOMAIN).map((e) => e.pointer + " " + e.rule);

test("a map spec of points, lines and polygons in range passes", () => {
  assert.deepEqual(mapRules(mapSpec()), []);
  const shapes = [
    { type: "Feature", geometry: { type: "LineString", coordinates: [[24.94, 60.17], [24.95, 60.18]] }, properties: { name: "Linka" } },
    { type: "Feature", geometry: { type: "MultiPolygon", coordinates: [[[[14.4, 50.0], [14.5, 50.0], [14.5, 50.1], [14.4, 50.0]]]] }, properties: { name: "Kontejner" } }
  ];
  assert.deepEqual(mapRules(mapSpec({ data: { type: "FeatureCollection", features: shapes } })), []);
});

test("a map spec is refused for a foreign link, a member of its own, a property it does not name or a position out of range", () => {
  assert.deepEqual(mapRules(mapSpec({ link: "https://evil.example/" })), ["/link a link only on our hosts"]);
  assert.deepEqual(mapRules(mapSpec({ link: "javascript:alert(1)" })), ["/link a link only on our hosts"]);
  assert.deepEqual(mapRules(mapSpec({ style: "https://evil.example/style.json" })), ["/style no member of that name"]);
  assert.deepEqual(mapRules(mapSpec({ data: { type: "FeatureCollection", features: [station([19.1, 48.7], { name: "x", secret: "y" })] } })),
    ["/data/features/0/properties/secret only the label, the popup fields and the time, as plain values"]);
  assert.deepEqual(mapRules(mapSpec({ data: { type: "FeatureCollection", features: [station([19.1, 48.7], { name: { html: "<b>" } })] } })),
    ["/data/features/0/properties/name only the label, the popup fields and the time, as plain values"]);
  assert.deepEqual(mapRules(mapSpec({ data: { type: "FeatureCollection", features: [station([190, 48.7])] } })),
    ["/data/features/0 a Point, LineString, Polygon or Multi* feature in range"]);
  assert.deepEqual(mapRules(mapSpec({ data: { type: "FeatureCollection", features: [{ type: "Feature", geometry: { type: "GeometryCollection", geometries: [] } }] } })),
    ["/data/features/0 a Point, LineString, Polygon or Multi* feature in range"]);
  assert.deepEqual(mapRules(mapSpec({ popup: [{ field: "pm10", label: "PM10", html: "<b>" }] })), ["/popup/0 a field, its label and its unit, nothing else",
    "/data/features/0/properties/pm10 only the label, the popup fields and the time, as plain values"]);
  assert.deepEqual(mapRules(mapSpec({ $schema: VL })), ["/$schema the map schema " + MAP]);
  assert.deepEqual(mapRules(mapSpec({ data: { type: "FeatureCollection", features: Array(1001).fill(station([19.1, 48.7])) } })),
    ["/data/features at most 1000 features"]);
  assert.deepEqual(mapRules([]), ["/ a spec is a JSON object"]);
});
