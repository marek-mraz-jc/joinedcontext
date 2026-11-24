// T-7591: one test per rule of the refusal list (ADR-N-053 §2.3 and §2.9, EP-130) in the file jc serves
// (vendor/viz/site/refuse.js): each refused spec beside the nearest spec the rule lets through.
//   node --test vendor/viz/tests/
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const refuse = createRequire(import.meta.url)("../site/refuse.js");
const DOMAIN = "dev.joinedcontext.com";
const VL = "https://vega.github.io/schema/vega-lite/v5.json";
const chart = (spec) => refuse(spec, DOMAIN).map((e) => `${e.pointer} ${e.rule}`);
const base = (more = {}) => ({ $schema: VL, data: { name: "source" }, mark: "point", ...more });
const map = (spec) => refuse.map(spec, DOMAIN).map((e) => `${e.pointer} ${e.rule}`);
const point = (properties = { name: "A" }, coordinates = [17.1, 48.1]) => ({ type: "Feature", geometry: { type: "Point", coordinates }, properties });
const mapSpec = (more = {}) => ({
  $schema: "urn:joinedcontext:map:v1",
  label: "name",
  popup: [{ field: "pm10", label: "PM10", unit: "µg/m³" }],
  data: { type: "FeatureCollection", features: [point()] },
  ...more,
});

test("chart: a spec is a JSON object", () => {
  for (const spec of [null, undefined, [], "x", 1, true]) assert.deepEqual(chart(spec), ["/ a spec is a JSON object"]);
  assert.deepEqual(chart(base()), []);
});

test("chart: a spec is plain JSON", () => {
  const loop = base();
  loop.self = loop;
  assert.deepEqual(chart(loop), ["/ a spec is plain JSON"]);
  assert.deepEqual(chart(base({ title: { text: 1n } })), ["/ a spec is plain JSON"]);
});

test("chart: at most 200 KB", () => {
  assert.ok(chart(base({ description: "x".repeat(200 * 1024) })).includes("/ at most 200 KB"));
  assert.deepEqual(chart(base({ description: "x".repeat(200 * 1024 - 200) })), []);
});

test("chart: the Vega-Lite 5 or Vega 5 schema", () => {
  for (const $schema of [undefined, "", "https://vega.github.io/schema/vega-lite/v6.json", "http://vega.github.io/schema/vega-lite/v5.json"]) {
    assert.deepEqual(chart(base({ $schema })), ["/$schema the Vega-Lite 5 or Vega 5 schema"]);
  }
  for (const $schema of ["https://vega.github.io/schema/vega-lite/v5.20.1.json", "https://vega.github.io/schema/vega/v5.json"]) assert.deepEqual(chart(base({ $schema })), []);
});

test("chart: no __proto__, constructor or prototype, as a key or in a string", () => {
  assert.deepEqual(chart(base({ transform: [{ calculate: "datum.constructor", as: "x" }] })), ["/transform/0/calculate no __proto__, constructor or prototype"]);
  assert.deepEqual(chart(base({ encoding: { prototype: { field: "x" } } })), ["/encoding/prototype no __proto__, constructor or prototype"]);
  assert.deepEqual(chart(base({ transform: [{ calculate: "datum.constructed", as: "x" }] })), []);
});

test("chart: no javascript: URL", () => {
  assert.deepEqual(chart(base({ title: "\tJAVASCRIPT:alert(1)" })), ["/title no javascript: URL"]);
  assert.deepEqual(chart(base({ title: "about javascript: urls" })), []);
});

test("chart: no usermeta", () => {
  assert.deepEqual(chart(base({ usermeta: {} })), ["/usermeta no usermeta"]);
});

test("chart: no loader options", () => {
  assert.deepEqual(chart(base({ config: { loader: {} } })), ["/config/loader no loader options"]);
});

test("chart: data only from the named source or inline values", () => {
  const no = "data only from the named source or inline values";
  for (const data of [null, "source", [{ name: "a" }, ["b"]], { name: "other" }, { url: "https://dev.joinedcontext.com/x" }, [{ name: "x", url: "y" }], [{ values: [] }]]) {
    assert.deepEqual(chart(base({ data })), [`/data ${no}`], JSON.stringify(data));
  }
  for (const data of [{ values: [{ a: 1 }] }, { sequence: { start: 0, stop: 3 } }, { graticule: true }, { sphere: true }, [{ name: "table", values: [] }]]) {
    assert.deepEqual(chart(base({ data })), [], JSON.stringify(data));
  }
});

test("chart: a url or an href only as a literal on our hosts", () => {
  const href = (value) => chart(base({ encoding: { href: value } }));
  const url = (value) => chart(base({ mark: "image", encoding: { url: value } }));
  for (const value of [
    { field: "link" },
    { value: "https://evil.example/" },
    { value: "https://x.dev.joinedcontext.com", extra: 1 },
    ["https://dev.joinedcontext.com/"],
    5,
    "https://app1.apps.dev.joinedcontext.com/",
    "http://dev.joinedcontext.com/",
    "https://user:pw@dev.joinedcontext.com/",
    "not a url",
    "data:image/svg+xml;base64,PHN2Zz4=",
  ]) {
    assert.deepEqual(href(value), ["/encoding/href a href only as a literal on our hosts"], JSON.stringify(value));
  }
  assert.deepEqual(url({ field: "picture" }), ["/encoding/url a url only as a literal on our hosts"]);
  for (const value of ["https://dev.joinedcontext.com/a", { value: "https://opendata.dev.joinedcontext.com/b" }]) assert.deepEqual(href(value), []);
  assert.deepEqual(url("data:image/png;base64,iVBORw0KGgo="), []);
});

test("chart: a key with ~ or / is named by its JSON pointer", () => {
  assert.deepEqual(chart(base({ "a/b~c": { usermeta: 1 } })), ["/a~1b~0c/usermeta no usermeta"]);
});

test("map: a spec is a JSON object", () => {
  for (const spec of [null, [], "x"]) assert.deepEqual(map(spec), ["/ a spec is a JSON object"]);
  assert.deepEqual(map(mapSpec()), []);
});

test("map: at most 8 MB", () => {
  assert.ok(map(mapSpec({ title: "x".repeat(8 * 1024 * 1024) })).includes("/ at most 8 MB"));
});

test("map: the map schema", () => {
  assert.deepEqual(map(mapSpec({ $schema: VL })), ["/$schema the map schema urn:joinedcontext:map:v1"]);
});

test("map: no member of another name", () => {
  assert.deepEqual(map(mapSpec({ style: "https://evil.example/style.json" })), ["/style no member of that name"]);
});

test("map: a title of at most 200 characters", () => {
  assert.deepEqual(map(mapSpec({ title: 7 })), ["/title a text of at most 200 characters"]);
  assert.deepEqual(map(mapSpec({ title: "x".repeat(201) })), ["/title a text of at most 200 characters"]);
  assert.deepEqual(map(mapSpec({ title: "x".repeat(200) })), []);
});

test("map: the name of the label property", () => {
  // Without a label name, the feature's `name` is no field the spec names either.
  assert.deepEqual(map(mapSpec({ label: 1 })), ["/label the name of the label property", "/data/features/0/properties/name only the label, the popup fields and the time, as plain values"]);
});

test("map: the name of the time property", () => {
  assert.deepEqual(map(mapSpec({ time: 1 })), ["/time the name of the time property"]);
  assert.deepEqual(map(mapSpec({ time: "at", data: { type: "FeatureCollection", features: [point({ name: "A", at: "2026-11-23" })] } })), []);
});

test("map: a link only on our hosts", () => {
  assert.deepEqual(map(mapSpec({ link: "https://evil.example/" })), ["/link a link only on our hosts"]);
  assert.deepEqual(map(mapSpec({ link: "https://dev.joinedcontext.com/datasets/air" })), []);
});

test("map: a list of at most 10 popup fields", () => {
  const field = (i) => ({ field: `f${i}`, label: `F${i}` });
  assert.deepEqual(map(mapSpec({ popup: "pm10" })), ["/popup a list of at most 10 fields"]);
  assert.deepEqual(map(mapSpec({ popup: Array.from({ length: 11 }, (_, i) => field(i)) })), ["/popup a list of at most 10 fields"]);
  assert.deepEqual(map(mapSpec({ popup: Array.from({ length: 10 }, (_, i) => field(i)) })), []);
});

test("map: a popup field, its label and its unit, nothing else", () => {
  for (const f of [null, { field: "pm10" }, { field: "pm10", label: "PM10", unit: 3 }, { field: "pm10", label: "PM10", style: "x" }]) {
    assert.deepEqual(map(mapSpec({ popup: [f] })), ["/popup/0 a field, its label and its unit, nothing else"], JSON.stringify(f));
  }
  assert.deepEqual(map(mapSpec({ popup: [{ field: "pm10", label: "PM10", unit: null }] })), []);
});

test("map: a GeoJSON FeatureCollection", () => {
  for (const data of [undefined, { type: "Feature", features: [] }, { type: "FeatureCollection", features: {} }, { type: "FeatureCollection", features: [], bbox: [] }]) {
    assert.deepEqual(map(mapSpec({ data })), ["/data a GeoJSON FeatureCollection"], JSON.stringify(data));
  }
});

test("map: the total and how many of it have no location, as whole numbers", () => {
  const no = ["/count the total and how many of it have no location, as whole numbers"];
  for (const count of [null, [], 5, { total: 0 }, { total: 1.5 }, { total: -1 }, { total: 10, unlocated: 10 }, { total: 10, unlocated: "3" }, { total: 10, shown: 1 }]) {
    assert.deepEqual(map(mapSpec({ count })), no, JSON.stringify(count));
  }
  for (const count of [{ total: 1 }, { total: 10, unlocated: 9 }, { total: 10, unlocated: 0 }]) assert.deepEqual(map(mapSpec({ count })), [], JSON.stringify(count));
});

test("map: at most 20000 features", () => {
  const features = Array.from({ length: 20001 }, () => point());
  assert.deepEqual(map(mapSpec({ data: { type: "FeatureCollection", features } })), ["/data/features at most 20000 features"]);
  assert.deepEqual(map(mapSpec({ data: { type: "FeatureCollection", features: features.slice(1) } })), []);
});

test("map: a Point, LineString, Polygon or Multi* feature in range", () => {
  const rule = "a Point, LineString, Polygon or Multi* feature in range";
  for (const f of [
    null,
    { type: "Thing", geometry: { type: "Point", coordinates: [1, 1] } },
    { type: "Feature" },
    { type: "Feature", geometry: { type: "GeometryCollection", geometries: [] } },
    point({ name: "A" }, [181, 0]),
    point({ name: "A" }, [0, -91]),
    point({ name: "A" }, [0]),
    point({ name: "A" }, ["0", 0]),
    { type: "Feature", geometry: { type: "LineString", coordinates: [] } },
    { type: "Feature", geometry: { type: "Polygon", coordinates: "x" } },
  ]) {
    assert.deepEqual(map(mapSpec({ data: { type: "FeatureCollection", features: [f] } })), [`/data/features/0 ${rule}`], JSON.stringify(f));
  }
  // A feature with no properties has nothing to refuse there.
  assert.deepEqual(map(mapSpec({ data: { type: "FeatureCollection", features: [{ type: "Feature", geometry: { type: "Point", coordinates: [0, 0] } }] } })), []);
});

test("map: only the label, the popup fields and the time, as plain values", () => {
  const rule = (k) => [`/data/features/0/properties/${k} only the label, the popup fields and the time, as plain values`];
  const one = (properties) => map(mapSpec({ data: { type: "FeatureCollection", features: [point(properties)] } }));
  assert.deepEqual(one({ name: "A", secret: "x" }), rule("secret"));
  assert.deepEqual(one({ name: { html: "<b>" } }), rule("name"));
  assert.deepEqual(one({ name: "A", pm10: Infinity }), rule("pm10"));
  assert.deepEqual(one({ name: null, pm10: 42 }), []);
  assert.deepEqual(one({ name: true, pm10: 4.2 }), []);
});
