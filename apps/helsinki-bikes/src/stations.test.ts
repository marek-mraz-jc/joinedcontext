import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import { STATIONS } from "./fixtures/stations";
import { BANDS, SHARE, bikes, hasBikes, histogram, isEmpty, ranked, share, totals, withShare } from "./stations";

const row = (availableBikeNumber: unknown): Row => ({ id: "urn:x", type: "BikeHireDockingStation", availableBikeNumber }) as Row;

describe("stations", () => {
  // AP-07: the tiles count what the feed says, and a station that says nothing is not empty.
  it("totals the five sampled stations", () => {
    expect(totals(STATIONS)).toEqual({ stations: 5, bikes: 15, empty: 2 });
    expect(totals([])).toEqual({ stations: 0, bikes: 0, empty: 0 });
  });

  it("reads a missing, negative or non-numeric count as unknown", () => {
    for (const value of [undefined, null, -1, "3", Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(bikes(row(value)), String(value)).toBeNull();
      expect(isEmpty(row(value)), String(value)).toBe(false);
      expect(hasBikes(row(value)), String(value)).toBe(false);
    }
    expect(isEmpty(row(0))).toBe(true);
    expect(hasBikes(row(1))).toBe(true);
  });

  // T-2924: the map colours by the share of docks with a bike; a station missing a count has none.
  it("computes the share of docks holding a bike, and none without both counts", () => {
    const [kaivopuisto, laivasillankatu, kapteeninpuistikko, viiskulma] = STATIONS;
    expect(share(kaivopuisto)).toBeCloseTo(4 / 30);
    expect(share(laivasillankatu)).toBe(0);
    expect(share(viiskulma)).toBeNull();
    expect(share({ ...kapteeninpuistikko, totalSlotNumber: 0 })).toBeNull();
    expect(share({ ...kapteeninpuistikko, availableBikeNumber: 40, totalSlotNumber: 16 })).toBe(1);
    const shaded = withShare(STATIONS);
    expect(shaded[0][SHARE]).toBe(13);
    expect(SHARE in shaded[3]).toBe(false);
  });

  it("puts each station with a count in one band of the histogram", () => {
    expect(histogram(STATIONS)).toEqual([2, 0, 1, 0, 1, 0]);
    expect(histogram([])).toEqual(BANDS.map(() => 0));
    expect(histogram([row(1), row(2), row(21), row(500), row(undefined)])).toEqual([0, 2, 0, 0, 0, 2]);
  });

  it("ranks the fullest and the emptiest stations, leaving out one without counts", () => {
    expect(ranked(STATIONS, 2, "fullest")).toEqual([
      { name: "Kapteeninpuistikko", percent: 69, bikes: 11 },
      { name: "Kaivopuisto", percent: 13, bikes: 4 },
    ]);
    expect(ranked(STATIONS, 10, "emptiest").map((s) => s.name)).toEqual([
      "Laivasillankatu",
      "Sepänkatu",
      "Kaivopuisto",
      "Kapteeninpuistikko",
    ]);
    expect(ranked(STATIONS, 0, "fullest")).toEqual([]);
    expect(ranked([], 10, "emptiest")).toEqual([]);
  });
});
