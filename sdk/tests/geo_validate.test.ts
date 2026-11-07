import { describe, expect, it } from "vitest";
import { MAX_VERTICES, checkGeometry, geometryOf } from "../src/geo/validate";
import type { GeometryType } from "../src/geo/validate";

/**
 * T-1442, UI-72, SDK-29: the check between a person's drawing and the write. Pure — no map, no
 * library — so the same answer holds in the editor's toolbar, in a paste of somebody else's file
 * and here.
 */
describe("the geometry a GeoProperty may hold", () => {
  const square = [
    [
      [0, 0],
      [1, 0],
      [1, 1],
      [0, 1],
      [0, 0],
    ],
  ];

  it("closes a ring the person left open, because dragging the last vertex onto the first means that", () => {
    const { geometry, findings } = checkGeometry({
      type: "Polygon",
      coordinates: [
        [
          [0, 0],
          [1, 0],
          [1, 1],
          [0, 1],
        ],
      ],
    });
    expect(findings).toEqual([]);
    expect((geometry?.coordinates as number[][][])[0]).toHaveLength(5);
    expect((geometry?.coordinates as number[][][])[0][4]).toEqual([0, 0]);
  });

  it("refuses a bow tie and names the two edges that cross", () => {
    const { geometry, findings } = checkGeometry({
      type: "Polygon",
      coordinates: [
        [
          [0, 0],
          [1, 1],
          [1, 0],
          [0, 1],
          [0, 0],
        ],
      ],
    });
    expect(geometry).toBeNull();
    expect(findings).toHaveLength(1);
    expect(findings[0].path).toBe("/coordinates/0");
    expect(findings[0].message).toMatch(/edge from point 0 crosses the edge from point 2/);
  });

  it("normalises the winding silently: no drawing tool asks which way round a person drew", () => {
    const clockwise = [
      [
        [0, 0],
        [0, 1],
        [1, 1],
        [1, 0],
        [0, 0],
      ],
    ];
    const { geometry, findings } = checkGeometry({ type: "Polygon", coordinates: clockwise });
    expect(findings).toEqual([]);
    // Counter-clockwise, as RFC 7946 §3.1.6 asks of an exterior ring.
    expect((geometry?.coordinates as number[][][])[0]).toEqual(square[0]);
  });

  it("flags a pair that is legal the other way round instead of only naming the bound", () => {
    // A Helsinki station is [24.9, 60.2]; the other way round is in the Arabian Sea.
    const { geometry, findings } = checkGeometry({ type: "Point", coordinates: [95.4, 24.9] });
    expect(geometry).not.toBeNull();
    expect(findings).toEqual([]);
    const swapped = checkGeometry({ type: "Point", coordinates: [60.2, 124.9] });
    expect(swapped.geometry).toBeNull();
    expect(swapped.findings[0].message).toMatch(/look swapped: longitude comes first/);
  });

  it("names the bound when swapping would not help either", () => {
    const { findings } = checkGeometry({ type: "Point", coordinates: [200, 100] });
    expect(findings.map((f) => f.path)).toEqual(["/coordinates/0", "/coordinates/1"]);
    expect(findings[0].message).toMatch(/longitude 200 is outside ±180/);
    expect(findings[1].message).toMatch(/latitude 100 is outside ±90/);
  });

  it("refuses a hole that is not inside its shell", () => {
    const { geometry, findings } = checkGeometry({
      type: "Polygon",
      coordinates: [
        square[0],
        [
          [5, 5],
          [5, 6],
          [6, 6],
          [5, 5],
        ],
      ],
    });
    expect(geometry).toBeNull();
    expect(findings.some((f) => f.message === "this hole is not inside the outer ring")).toBe(true);
  });

  it("takes a hole that is inside it", () => {
    const { geometry, findings } = checkGeometry({
      type: "Polygon",
      coordinates: [
        [
          [0, 0],
          [4, 0],
          [4, 4],
          [0, 4],
          [0, 0],
        ],
        [
          [1, 1],
          [1, 2],
          [2, 2],
          [1, 1],
        ],
      ],
    });
    expect(findings).toEqual([]);
    expect((geometry?.coordinates as number[][][])).toHaveLength(2);
  });

  it("refuses a geometry the editor does not allow, rather than converting it", () => {
    const allowed: GeometryType[] = ["Polygon", "MultiPolygon"];
    const { geometry, findings } = checkGeometry({ type: "Point", coordinates: [1, 2] }, allowed);
    expect(geometry).toBeNull();
    expect(findings[0].message).toBe("Point is not one of Polygon, MultiPolygon here");
  });

  it(`refuses ${MAX_VERTICES + 1} points: a drawing that large is a paste of something else`, () => {
    const many = Array.from({ length: MAX_VERTICES + 1 }, (_, i) => [i / 1000, 0]);
    const { geometry, findings } = checkGeometry({ type: "MultiPoint", coordinates: many });
    expect(geometry).toBeNull();
    expect(findings[0].message).toMatch(new RegExp(`${MAX_VERTICES + 1} points is more than`));
  });

  it("calls an empty editor and a cleared attribute no geometry, never an error", () => {
    for (const nothing of [null, undefined, { type: "MultiPoint", coordinates: [] }]) {
      const { geometry, findings } = checkGeometry(nothing);
      expect(geometry).toBeNull();
      expect(findings).toEqual([]);
    }
  });

  it("keeps an altitude it was given", () => {
    const { geometry, findings } = checkGeometry({ type: "Point", coordinates: [24.9, 60.2, 17.5] });
    expect(findings).toEqual([]);
    expect(geometry?.coordinates).toEqual([24.9, 60.2, 17.5]);
  });

  it("refuses a line of one point and a ring of two corners", () => {
    expect(checkGeometry({ type: "LineString", coordinates: [[1, 2]] }).geometry).toBeNull();
    expect(
      checkGeometry({
        type: "Polygon",
        coordinates: [
          [
            [0, 0],
            [1, 1],
          ],
        ],
      }).geometry,
    ).toBeNull();
  });

  it("refuses a ring whose corners are all on one line", () => {
    const { geometry, findings } = checkGeometry({
      type: "Polygon",
      coordinates: [
        [
          [0, 0],
          [1, 1],
          [2, 2],
          [0, 0],
        ],
      ],
    });
    expect(geometry).toBeNull();
    expect(findings[0].message).toMatch(/encloses nothing/);
  });

  it("says what is wrong with something that is not GeoJSON at all", () => {
    expect(checkGeometry("POINT(1 2)").findings[0].message).toBe("must be a GeoJSON geometry");
    expect(checkGeometry({ type: "Sphere", coordinates: [] }).findings[0].path).toBe("/type");
  });
});

describe("what a paste or an upload carries", () => {
  const point = (x: number) => ({ type: "Feature", geometry: { type: "Point", coordinates: [x, 1] } });

  it("takes the geometry out of a Feature", () => {
    const { geometry, findings } = geometryOf(point(3));
    expect(findings).toEqual([]);
    expect(geometry).toEqual({ type: "Point", coordinates: [3, 1] });
  });

  it("makes a collection of points a MultiPoint where that is allowed", () => {
    const { geometry, findings } = geometryOf(
      { type: "FeatureCollection", features: [point(1), point(2)] },
      ["MultiPoint"],
    );
    expect(findings).toEqual([]);
    expect(geometry).toEqual({
      type: "MultiPoint",
      coordinates: [
        [1, 1],
        [2, 1],
      ],
    });
  });

  it("keeps the first geometry and says so when the Multi- form is not allowed", () => {
    const { geometry, findings } = geometryOf(
      { type: "FeatureCollection", features: [point(1), point(2)] },
      ["Point"],
    );
    expect(geometry).toEqual({ type: "Point", coordinates: [1, 1] });
    expect(findings).toEqual([
      { path: "/features", message: "2 geometries were given and only the first one is kept here" },
    ]);
  });

  it("says a collection with no geometry carries none", () => {
    const { geometry, findings } = geometryOf({ type: "FeatureCollection", features: [] });
    expect(geometry).toBeNull();
    expect(findings[0].message).toBe("carries no geometry");
  });
});
