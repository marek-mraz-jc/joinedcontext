// T-7755: a map of every matching entity, up to 20,000 points, bounded in one pass and clustered.
//   node --test vendor/viz/tests/
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const map = createRequire(import.meta.url)("../site/map.js");
const point = (id, at) => ({ type: "Feature", id, geometry: { type: "Point", coordinates: at }, properties: {} });

test("bounds holds every position, nested ones too", () => {
  const line = { type: "Feature", id: 2, geometry: { type: "LineString", coordinates: [[17, 47.5], [22.6, 49.6]] } };
  assert.deepEqual(map.bounds([point(0, [19.1, 48.7]), point(1, [18, 48]), line]), [[17, 47.5], [22.6, 49.6]]);
  assert.equal(map.bounds([]), null);
});

test("bounds of 20,000 points takes one pass", () => {
  const many = Array.from({ length: 20000 }, (_, i) => point(i, [17 + (i % 5), 47 + (i % 3)]));
  const began = Date.now();
  assert.deepEqual(map.bounds(many), [[17, 47], [21, 49]]);
  assert.ok(Date.now() - began < 1000);
});

test("past 200 points the points cluster, each with its feature's index; lines stay out", () => {
  const few = Array.from({ length: 200 }, (_, i) => point(i, [19, 48]));
  assert.equal(map.clustered(few), null);
  const line = { type: "Feature", id: 201, geometry: { type: "LineString", coordinates: [[17, 47], [18, 48]] } };
  const many = few.concat([point(200, [20, 49]), line]);
  const pts = map.clustered(many);
  assert.equal(pts.length, 201);
  assert.deepEqual(pts[200], { type: "Feature", geometry: { type: "Point", coordinates: [20, 49] }, properties: { jcIndex: 200 } });
});

test("a year in a popup reads as a year (T-7755)", () => {
  const spec = { label: "name", popup: [{ field: "yearBuilt", label: "Rok" }, { field: "length", label: "Dĺžka", unit: "m" }] };
  const said = map.lines(spec, { name: "Most", yearBuilt: 1910, length: 1250.5 }, "sk");
  assert.equal(said.rows[0][1], "1910");
  assert.equal(said.rows[1][1], new Intl.NumberFormat("sk", { maximumFractionDigits: 3 }).format(1250.5) + " m");
});
