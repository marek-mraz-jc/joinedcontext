/**
 * The rows of a page as shapes, and what a pending geometry changed (UI-72, UI-67).
 */
import { describe, expect, it } from "vitest";

import {
  diffGeometry,
  featuresOfRows,
  geometryOfRow,
  mapAttrOf,
  metresBetween,
  rowsOffTheMap,
  verticesOf,
} from "../src/grid/mapRows";
import type { Geometry } from "../src/geo/validate";
import type { RichRow } from "../src/grid/model";

function station(id: string, location: unknown, extra: Record<string, unknown> = {}): RichRow {
  return {
    id,
    type: "BikeHireDockingStation",
    cells: {
      name: { kind: "property", value: id },
      ...(location === undefined ? {} : { location: { kind: "geo", value: location, geometryType: "Point" } }),
      ...extra,
    } as RichRow["cells"],
    raw: {},
  };
}

const point = (lon: number, lat: number): Geometry => ({ type: "Point", coordinates: [lon, lat] }) as Geometry;

describe("which attribute the map is about", () => {
  it("takes the first attribute of the page that holds a geometry", () => {
    const rows = [station("a", undefined), station("b", point(24.9, 60.1))];
    expect(mapAttrOf(rows)).toBe("location");
  });

  it("is nothing at all for a type with no geometry, so there is no map", () => {
    expect(mapAttrOf([station("a", undefined)])).toBeNull();
    expect(mapAttrOf([])).toBeNull();
  });

  it("keeps the attribute the config named even when this page has none of it", () => {
    // Otherwise a filter that hid every station would move the panel to another attribute under
    // the person's hands.
    expect(mapAttrOf([station("a", undefined)], "location")).toBe("location");
  });
});

describe("the rows as features", () => {
  it("is one feature per row that has a geometry, keyed by the entity id", () => {
    const rows = [station("a", point(24.9, 60.1)), station("b", undefined), station("c", point(25, 60.2))];
    const features = featuresOfRows(rows, "location");
    expect(features.map((f) => f.id)).toEqual(["a", "c"]);
    expect(features[0].geometry).toEqual(point(24.9, 60.1));
  });

  it("lists the rows that are not on the map instead of hiding them", () => {
    const rows = [station("a", point(24.9, 60.1)), station("b", undefined), station("c", undefined)];
    expect(rowsOffTheMap(rows, "location")).toEqual(["b", "c"]);
  });

  it("shows a pending geometry, so the panel and the cell hold the same value", () => {
    const rows = [station("a", point(24.9, 60.1)), station("b", undefined)];
    const edits = { a: { location: point(25, 60.3) }, b: { location: point(24.8, 60.05) } };
    const features = featuresOfRows(rows, "location", edits);
    expect(features.map((f) => f.id)).toEqual(["a", "b"]);
    expect(features[0].geometry).toEqual(point(25, 60.3));
    expect(features[0].properties?.edited).toBe(true);
    // The row that had nothing is on the map now, so it is no longer listed as off it.
    expect(rowsOffTheMap(rows, "location", edits)).toEqual([]);
  });

  it("refuses a cell that is not a geometry rather than drawing whatever it holds", () => {
    const wrong = station("a", undefined, { location: { kind: "property", value: "somewhere" } });
    expect(geometryOfRow(wrong, "location")).toBeNull();
    const half = station("b", { type: "Point" });
    expect(geometryOfRow(half, "location")).toBeNull();
    expect(featuresOfRows([wrong, half], "location")).toEqual([]);
  });
});

describe("what a pending geometry changed", () => {
  it("says how far a point moved, in metres", () => {
    // One ten-thousandth of a degree of latitude is ~11.1 m.
    const diff = diffGeometry(point(24.9, 60.1), point(24.9, 60.1001));
    expect(diff.kind).toBe("moved");
    expect(diff.metres).toBeGreaterThan(10);
    expect(diff.metres).toBeLessThan(12);
  });

  it("says how many vertices and how much area a polygon gained", () => {
    const before = {
      type: "Polygon",
      coordinates: [[[0, 0], [0.001, 0], [0.001, 0.001], [0, 0.001], [0, 0]]],
    } as Geometry;
    const after = {
      type: "Polygon",
      coordinates: [[[0, 0], [0.001, 0], [0.0012, 0.0005], [0.001, 0.001], [0, 0.001], [0, 0]]],
    } as Geometry;
    const diff = diffGeometry(before, after);
    expect(diff.kind).toBe("reshaped");
    expect(diff.vertices).toBe(1);
    expect(diff.areaPercent).toBeGreaterThan(0);
  });

  it("does not pretend to measure between two different kinds of shape", () => {
    const line = { type: "LineString", coordinates: [[0, 0], [1, 1]] } as Geometry;
    expect(diffGeometry(point(0, 0), line)).toEqual({ kind: "replaced", from: "Point", to: "LineString" });
    expect(diffGeometry(null, point(0, 0)).kind).toBe("replaced");
    expect(diffGeometry(point(0, 0), null).kind).toBe("replaced");
  });

  it("says nothing changed when nothing did", () => {
    expect(diffGeometry(point(24.9, 60.1), point(24.9, 60.1))).toEqual({ kind: "same" });
  });

  it("reads the vertices of any nesting, and measures zero for the same position", () => {
    const polygon = { type: "Polygon", coordinates: [[[0, 0], [1, 0], [1, 1], [0, 0]]] } as Geometry;
    expect(verticesOf(polygon)).toHaveLength(4);
    expect(verticesOf(null)).toEqual([]);
    expect(metresBetween([24.9, 60.1], [24.9, 60.1])).toBe(0);
  });
});
