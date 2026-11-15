import { describe, expect, it } from "vitest";
import { dueAgainst, readingAt } from "./earlier";
import type { KeptForecast } from "./server";

const DAY = 86_400_000;
const T = Date.UTC(2030, 9, 10, 12);
const forecast = (madeOn: string, points: Array<[number, number, number, number]>): KeptForecast => ({
  madeOn,
  days: 30,
  step: DAY,
  latestT: T - DAY,
  latestV: 10,
  direction: "up",
  points: points.map(([t, v, lo, hi]) => ({ t, v, lo, hi })),
});

// T-3350: an earlier forecast set against what was measured.
describe("dueAgainst", () => {
  it("takes the reading within half a step, the nearest when there are two", () => {
    const history: Array<[number, number]> = [
      [T - 0.6 * DAY, 1],
      [T + 0.2 * DAY, 2],
      [T - 0.1 * DAY, 3],
    ];
    expect(readingAt(history, T, DAY)).toBe(3);
    expect(readingAt(history, T + 3 * DAY, DAY)).toBeNull();
    expect(readingAt([], T, DAY)).toBeNull();
  });

  it("keeps the points that have come due, newest first, each inside, outside or unread", () => {
    const kept = [
      forecast("2030-10-09", [
        [T, 10, 8, 12],
        [T + DAY, 11, 9, 13],
        [T + 2 * DAY, 12, 10, 14],
      ]),
      forecast("2030-10-08", [[T - DAY, 9, 7, 11]]),
    ];
    const history: Array<[number, number]> = [
      [T, 11],
      [T + DAY, 20],
    ];
    const due = dueAgainst(kept, history, T + DAY);
    expect(due.map((d) => [d.madeOn, d.t, d.measured, d.within])).toEqual([
      ["2030-10-09", T + DAY, 20, false],
      ["2030-10-09", T, 11, true],
      ["2030-10-08", T - DAY, null, null],
    ]);
    expect(dueAgainst(kept, history, T + DAY, 1)).toHaveLength(1);
    expect(dueAgainst([], history, T)).toEqual([]);
  });
});
