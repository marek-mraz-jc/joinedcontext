// T-4657: the map page's own logic under node: what a popup says, the view it fits, what Enter opens.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const map = createRequire(import.meta.url)("../charts/renderer/files/site/map.js");
const spec = {
  label: "name", time: "observedAt", link: "https://opendata.dev.joinedcontext.com/dataset/ovzdusie",
  popup: [{ field: "no2", label: "NO₂", unit: "µg/m³" }, { field: "pm10", label: "PM10", unit: "µg/m³" }, { field: "open", label: "Open" }]
};

test("a popup says the label, each value with its unit, the time and a link, as plain text", () => {
  const said = map.lines(spec, { name: "SK0214A <img src=x onerror=alert(1)>", no2: 14.25, pm10: null, open: true, observedAt: "2026-10-18T15:00:00Z" }, "sk");
  assert.equal(said.title, "SK0214A <img src=x onerror=alert(1)>", "markup stays text");
  assert.deepEqual(said.rows, [["NO₂", "14,25 µg/m³"], ["PM10", "–"], ["Open", "áno"]]);
  assert.match(said.time, /2026/);
  assert.deepEqual(said.link, { href: spec.link, text: "Detail" });
  const bare = map.lines({ ...spec, link: undefined, time: undefined }, { name: "x" }, "en");
  assert.equal(bare.link, null);
  assert.equal(bare.time, null);
});

test("the view fits every position, and Enter opens the feature nearest the middle", () => {
  const features = [
    { geometry: { type: "Point", coordinates: [19.14, 48.73] } },
    { geometry: { type: "LineString", coordinates: [[19.0, 48.6], [19.3, 48.9]] } },
    { geometry: { type: "Polygon", coordinates: [[[18.9, 48.5], [19.0, 48.5], [19.0, 48.6], [18.9, 48.5]]] } }
  ];
  assert.deepEqual(map.bounds(features), [[18.9, 48.5], [19.3, 48.9]]);
  assert.equal(map.bounds([]), null);
  assert.equal(map.nearest(features, [19.15, 48.74]), 0);
  assert.equal(map.nearest(features, [18.91, 48.51]), 2);
  assert.equal(map.nearest([], [0, 0]), -1);
});

test("every line and polygon gets a point on itself, with its id; a point gets none (T-4727)", () => {
  const features = [
    { id: 0, geometry: { type: "Point", coordinates: [19.14, 48.73] }, properties: { name: "a" } },
    { id: 1, geometry: { type: "LineString", coordinates: [[19.0, 48.6], [19.1, 48.7], [19.3, 48.9]] }, properties: { name: "bridge" } },
    { id: 2, geometry: { type: "MultiPolygon", coordinates: [[[[18.9, 48.5], [19.0, 48.5], [19.0, 48.6], [18.9, 48.5]]]] }, properties: {} },
    { id: 3, geometry: { type: "MultiPoint", coordinates: [[19.2, 48.8]] }, properties: {} }
  ];
  const got = map.anchors(features);
  assert.deepEqual(got.map((f) => f.id), [1, 2]);
  assert.deepEqual(got[0].geometry, { type: "Point", coordinates: [19.1, 48.7] });
  assert.equal(got[0].properties.name, "bridge", "the anchor opens the feature's own popup");
  assert.deepEqual(got[1].geometry.coordinates, [19.0, 48.6]);
  assert.deepEqual(map.anchors([]), []);
});
