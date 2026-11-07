import { describe, expect, it } from "vitest";
import { indicators, inPeriod, standing, trend } from "./indicators";
import { ROWS } from "./fixtures";

const named = (months: number, name: string) => indicators(inPeriod(ROWS, months)).find((item) => item.name === name)!;

describe("the indicators", () => {
  it("counts the period back from the newest observation, whole months", () => {
    expect(inPeriod(ROWS, 3)).toHaveLength(5 * 3);
    expect(inPeriod(ROWS, 12)).toHaveLength(ROWS.length);
    expect(inPeriod([], 6)).toEqual([]);
    expect(inPeriod([{ id: "urn:x", type: "KeyPerformanceIndicator", name: "Undated" }], 6)).toEqual([]);
  });

  it("groups by name, oldest first, and skips an observation without a name", () => {
    const list = indicators([...ROWS, { id: "urn:y", type: "KeyPerformanceIndicator", kpiValue: 1 }]);
    expect(list.map((item) => item.name)).toEqual([
      "PM2.5 mean",
      "Recycling rate",
      "Bus punctuality",
      "Cycling share of trips",
      "Resident satisfaction",
    ]);
    const cycling = list[3];
    expect(cycling.points).toHaveLength(12);
    expect(cycling.latest).toBe(19.2);
    expect(cycling.target).toBe(20);
  });

  it("reads the standing in the indicator's own direction", () => {
    expect(standing(named(12, "Cycling share of trips"))).toEqual({ kind: "short", by: expect.closeTo(0.8) });
    expect(standing(named(12, "PM2.5 mean"))).toEqual({ kind: "on-target", by: expect.closeTo(0.6) });
    expect(standing({ latest: 10, target: 10, higherIsBetter: false })).toEqual({ kind: "on-target", by: 0 });
    expect(standing(named(12, "Resident satisfaction"))).toEqual({ kind: "unknown" });
    expect(standing({ latest: null, target: 5, higherIsBetter: true })).toEqual({ kind: "unknown" });
  });

  it("reads the trend over the chosen period, and none from a single point", () => {
    expect(trend(named(12, "PM2.5 mean"))).toBe("better");
    expect(trend(named(12, "Recycling rate"))).toBe("worse");
    expect(trend(named(12, "Bus punctuality"))).toBe("level");
    expect(trend(named(6, "Bus punctuality"))).toBe("worse");
    expect(trend(named(3, "Bus punctuality"))).toBe("better");
    expect(trend({ points: ROWS.slice(0, 1), higherIsBetter: true })).toBe("unknown");
  });
});
