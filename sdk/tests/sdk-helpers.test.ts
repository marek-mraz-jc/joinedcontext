import { describe, expect, it } from "vitest";
import type { Row } from "../src/ngsi";
import { groupBy, toFeatureCollection } from "../src/sdk/helpers";

describe("groupBy", () => {
  const sampleRows: Row[] = [
    { id: "1", type: "Station", city: "Helsinki", bikes: 10 },
    { id: "2", type: "Station", city: "Helsinki", bikes: 20 },
    { id: "3", type: "Station", city: "Espoo", bikes: 15 },
    { id: "4", type: "Station", city: "", bikes: 5 },
    { id: "5", type: "Station", city: null, bikes: 8 },
    { id: "6", type: "Station", city: "Vantaa", bikes: null },
  ];

  it("aggregates count by default", () => {
    const groups = groupBy(sampleRows, "city");
    expect(groups).toEqual([
      { key: "(none)", value: 2 },
      { key: "Helsinki", value: 2 },
      { key: "Espoo", value: 1 },
      { key: "Vantaa", value: 1 },
    ]);
  });

  it("aggregates sum and avg over valueAttr", () => {
    const sums = groupBy(sampleRows, "city", "sum", "bikes");
    expect(sums).toEqual([
      { key: "Helsinki", value: 30 },
      { key: "Espoo", value: 15 },
      { key: "(none)", value: 13 },
    ]);

    const avgs = groupBy(sampleRows, "city", "avg", "bikes");
    expect(avgs).toEqual([
      { key: "Espoo", value: 15 },
      { key: "Helsinki", value: 15 },
      { key: "(none)", value: 6.5 },
    ]);
  });

  it("drops groups with a null aggregate (e.g. avg of no numbers)", () => {
    const rows: Row[] = [
      { id: "1", type: "T", category: "A", val: null },
      { id: "2", type: "T", category: "B", val: 4 },
    ];
    const res = groupBy(rows, "category", "avg", "val");
    expect(res).toEqual([{ key: "B", value: 4 }]);
  });

  it("sorts by value descending, then key ascending for ties", () => {
    const rows: Row[] = [
      { id: "1", type: "T", tag: "Z", score: 10 },
      { id: "2", type: "T", tag: "A", score: 10 },
      { id: "3", type: "T", tag: "M", score: 20 },
    ];
    const groups = groupBy(rows, "tag", "sum", "score");
    expect(groups).toEqual([
      { key: "M", value: 20 },
      { key: "A", value: 10 },
      { key: "Z", value: 10 },
    ]);
  });

  it("respects top parameter when > 0, keeps all when top <= 0 or undefined", () => {
    const rows: Row[] = [
      { id: "1", type: "T", c: "1", v: 10 },
      { id: "2", type: "T", c: "2", v: 20 },
      { id: "3", type: "T", c: "3", v: 30 },
    ];
    expect(groupBy(rows, "c", "sum", "v", 2)).toHaveLength(2);
    expect(groupBy(rows, "c", "sum", "v", 0)).toHaveLength(3);
    expect(groupBy(rows, "c", "sum", "v", -1)).toHaveLength(3);
    expect(groupBy(rows, "c", "sum", "v", undefined)).toHaveLength(3);
  });
});

describe("toFeatureCollection", () => {
  it("keeps polygons, skips rows without geometry, and includes properties", () => {
    const rows: Row[] = [
      {
        id: "urn:1",
        type: "Zone",
        location: {
          type: "Polygon",
          coordinates: [
            [
              [24.9, 60.1],
              [24.95, 60.1],
              [24.95, 60.15],
              [24.9, 60.1],
            ],
          ],
        },
        name: "Downtown",
        active: true,
      },
      {
        id: "urn:2",
        type: "Station",
        location: {
          type: "Point",
          coordinates: [24.93, 60.16],
        },
        name: "Kamppi",
        bikes: 12,
      },
      {
        id: "urn:3",
        type: "Station",
        location: null,
        name: "No Geo",
      },
      {
        id: "urn:4",
        type: "Station",
        location: "invalid geo string",
        name: "Invalid Geo",
      },
    ];

    const fc = toFeatureCollection(rows);
    expect(fc.type).toBe("FeatureCollection");
    expect(fc.features).toHaveLength(2);

    expect(fc.features[0]).toEqual({
      type: "Feature",
      id: "urn:1",
      geometry: rows[0].location,
      properties: {
        id: "urn:1",
        type: "Zone",
        name: "Downtown",
        active: true,
      },
    });

    expect(fc.features[1]).toEqual({
      type: "Feature",
      id: "urn:2",
      geometry: rows[1].location,
      properties: {
        id: "urn:2",
        type: "Station",
        name: "Kamppi",
        bikes: 12,
      },
    });
  });

  it("filters properties when properties list is specified", () => {
    const rows: Row[] = [
      {
        id: "urn:1",
        type: "Station",
        location: { type: "Point", coordinates: [0, 0] },
        name: "Central",
        bikes: 20,
        extra: "hidden",
      },
    ];

    const fc = toFeatureCollection(rows, "location", ["name"]);
    expect(fc.features[0].properties).toEqual({
      id: "urn:1",
      type: "Station",
      name: "Central",
    });
  });
});
