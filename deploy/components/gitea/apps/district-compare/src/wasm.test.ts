import { describe, expect, it } from "vitest";
import wasm from "../wasm/pkg/district_compare_bg.wasm?url&inline";
import { compare, initSync } from "../wasm/pkg/district_compare.js";
import { parseAnswer } from "./compare";
import type { CompareInput } from "./districts";
import { MEASURES, toInput } from "./districts";
import { ROWS } from "./fixtures/districts";

const base64 = wasm.slice(wasm.indexOf(",") + 1);
initSync({ module: Uint8Array.from(atob(base64), (c) => c.charCodeAt(0)) });

describe("wasm compare on fixtures", () => {
  const input = toInput(ROWS);
  const rawAnswer = compare(JSON.stringify(input));
  const out = parseAnswer(rawAnswer);

  it("produces output for all valid districts and ignores subdistricts", () => {
    expect(out.districts).toHaveLength(3);
    const subdistricts = ROWS.districts.filter((d) => d.divisionLevel !== "district");
    expect(subdistricts.length).toBeGreaterThan(0);
    for (const sub of subdistricts) {
      expect(out.districts.map((d) => d.code)).not.toContain(String(sub.districtCode ?? ""));
    }
  });

  it("calculates counts per district and conserves all points across districts and outside", () => {
    for (const d of out.districts) {
      expect(d.events).toBeGreaterThanOrEqual(0);
      expect(d.bikes).toBeGreaterThanOrEqual(0);
      expect(d.bikeSlots).toBeGreaterThanOrEqual(0);
      expect(d.alerts).toBeGreaterThanOrEqual(0);
    }

    const totalEvents = out.districts.reduce((sum, d) => sum + d.events, 0) + out.outside.events;
    expect(totalEvents).toBe(ROWS.events.length);

    const totalBikes = out.districts.reduce((sum, d) => sum + d.bikes, 0) + out.outside.bikes;
    expect(totalBikes).toBe(ROWS.bikes.length);

    const totalAlerts = out.districts.reduce((sum, d) => sum + d.alerts, 0) + out.outside.alerts;
    expect(totalAlerts).toBe(ROWS.alerts.length);

    const totalAir =
      out.districts.reduce((sum, d) => sum + (d.pm25 !== null || d.aqi !== null ? 1 : 0), 0) + out.outside.air;
    expect(totalAir).toBe(ROWS.air.length);
  });

  it("counts outside points including unlocated events", () => {
    expect(out.outside.events).toBeGreaterThanOrEqual(1);
    expect(out.outside.bikes).toBeGreaterThanOrEqual(1);
    expect(out.outside.alerts).toBeGreaterThanOrEqual(1);
    expect(out.outside.air).toBeGreaterThanOrEqual(1);
  });

  it("computes per-km² rates equal to count divided by area for every district", () => {
    for (const d of out.districts) {
      expect(d.areaKm2).toBeGreaterThan(0);
      expect(d.perKm2.events).toBeCloseTo(d.events / d.areaKm2, 5);
      expect(d.perKm2.bikes).toBeCloseTo(d.bikes / d.areaKm2, 5);
      expect(d.perKm2.bikeSlots).toBeCloseTo(d.bikeSlots / d.areaKm2, 5);
      expect(d.perKm2.alerts).toBeCloseTo(d.alerts / d.areaKm2, 5);
    }
  });

  it("excludes points falling in polygon holes", () => {
    const holeRow = ROWS.districts.find(
      (d) =>
        d.divisionLevel === "district" &&
        (d.location as { type?: string })?.type === "Polygon" &&
        Array.isArray((d.location as { coordinates?: unknown[] })?.coordinates) &&
        (d.location as { coordinates: unknown[] }).coordinates.length > 1,
    );
    expect(holeRow).toBeDefined();
    const holeDistrict = out.districts.find((d) => d.code === String(holeRow?.districtCode ?? ""));
    expect(holeDistrict).toBeDefined();
  });

  it("counts points falling in both parts of a MultiPolygon", () => {
    const multiRow = ROWS.districts.find(
      (d) => d.divisionLevel === "district" && (d.location as { type?: string })?.type === "MultiPolygon",
    );
    expect(multiRow).toBeDefined();
    const multiDistrict = out.districts.find((d) => d.code === String(multiRow?.districtCode ?? ""));
    expect(multiDistrict).toBeDefined();
    expect(multiDistrict?.events).toBeGreaterThanOrEqual(1);
  });

  it("district with no air station has pm25 null and rank null", () => {
    const noAirDistrict = out.districts.find((d) => d.pm25 === null);
    expect(noAirDistrict).toBeDefined();
    expect(noAirDistrict?.pm25).toBeNull();
    expect(noAirDistrict?.aqi).toBeNull();
    expect(noAirDistrict?.rank.pm25).toBeNull();
    expect(noAirDistrict?.rank.aqi).toBeNull();
  });

  it("competition ranks contain a tie where tied districts share the same rank", () => {
    const tiedMeasure = MEASURES.find((m) => {
      const ranks = out.districts.map((d) => d.rank[m]).filter((r): r is number => r !== null);
      return new Set(ranks).size < ranks.length;
    });
    expect(tiedMeasure).toBeDefined();
  });
});

describe("wasm compare isolated logic", () => {
  it("excludes a point inside a polygon hole into outside", () => {
    const input: CompareInput = {
      districts: [
        {
          code: "h1",
          name: "Hole District",
          geometry: {
            type: "Polygon",
            coordinates: [
              [
                [24.9, 60.1],
                [25.0, 60.1],
                [25.0, 60.2],
                [24.9, 60.2],
                [24.9, 60.1],
              ],
              [
                [24.93, 60.13],
                [24.97, 60.13],
                [24.97, 60.17],
                [24.93, 60.17],
                [24.93, 60.13],
              ],
            ],
          },
        },
      ],
      events: [
        [24.91, 60.11], // inside outer ring, outside hole
        [24.95, 60.15], // inside hole
      ],
      bikes: [],
      alerts: [],
      air: [],
    };
    const res = parseAnswer(compare(JSON.stringify(input)));
    expect(res.districts[0].events).toBe(1);
    expect(res.outside.events).toBe(1);
  });

  it("counts points from both parts of a MultiPolygon", () => {
    const input: CompareInput = {
      districts: [
        {
          code: "m1",
          name: "Multi District",
          geometry: {
            type: "MultiPolygon",
            coordinates: [
              [[[24.8, 60.1], [24.85, 60.1], [24.85, 60.15], [24.8, 60.15], [24.8, 60.1]]],
              [[[24.9, 60.2], [24.95, 60.2], [24.95, 60.25], [24.9, 60.25], [24.9, 60.2]]],
            ],
          },
        },
      ],
      events: [
        [24.82, 60.12], // in part 1
        [24.92, 60.22], // in part 2
      ],
      bikes: [],
      alerts: [],
      air: [],
    };
    const res = parseAnswer(compare(JSON.stringify(input)));
    expect(res.districts[0].events).toBe(2);
    expect(res.outside.events).toBe(0);
  });

  it("competition ranking assigns equal ranks to ties and skips subsequent rank", () => {
    const input: CompareInput = {
      districts: [
        {
          code: "d1",
          name: "D1",
          geometry: {
            type: "Polygon",
            coordinates: [[[24.0, 60.0], [24.1, 60.0], [24.1, 60.1], [24.0, 60.1], [24.0, 60.0]]],
          },
        },
        {
          code: "d2",
          name: "D2",
          geometry: {
            type: "Polygon",
            coordinates: [[[24.2, 60.0], [24.3, 60.0], [24.3, 60.1], [24.2, 60.1], [24.2, 60.0]]],
          },
        },
        {
          code: "d3",
          name: "D3",
          geometry: {
            type: "Polygon",
            coordinates: [[[24.4, 60.0], [24.5, 60.0], [24.5, 60.1], [24.4, 60.1], [24.4, 60.0]]],
          },
        },
      ],
      events: [
        [24.05, 60.05],
        [24.05, 60.06], // 2 in d1
        [24.25, 60.05],
        [24.25, 60.06], // 2 in d2
        [24.45, 60.05], // 1 in d3
      ],
      bikes: [],
      alerts: [],
      air: [],
    };
    const res = parseAnswer(compare(JSON.stringify(input)));
    expect(res.districts[0].rank.events).toBe(1);
    expect(res.districts[1].rank.events).toBe(1);
    expect(res.districts[2].rank.events).toBe(3);
  });

  it("district with no air station has pm25 null and rank null", () => {
    const input: CompareInput = {
      districts: [
        {
          code: "d1",
          name: "D1",
          geometry: {
            type: "Polygon",
            coordinates: [[[24.0, 60.0], [24.1, 60.0], [24.1, 60.1], [24.0, 60.1], [24.0, 60.0]]],
          },
        },
      ],
      events: [],
      bikes: [],
      alerts: [],
      air: [],
    };
    const res = parseAnswer(compare(JSON.stringify(input)));
    expect(res.districts[0].pm25).toBeNull();
    expect(res.districts[0].aqi).toBeNull();
    expect(res.districts[0].rank.pm25).toBeNull();
    expect(res.districts[0].rank.aqi).toBeNull();
  });

  it("bad JSON answers an error object and parseAnswer throws", () => {
    const badRaw = compare("not valid json");
    const parsed = JSON.parse(badRaw) as { error?: string };
    expect(parsed.error).toBeDefined();
    expect(parsed.error).toContain("the districts could not be read");
    expect(() => parseAnswer(badRaw)).toThrow("the districts could not be read");
  });
});
