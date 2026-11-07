import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import { ALERT_LEVEL, crossings, merge, record, seed, silentFor, sparkline } from "./monitor";

const row = (id: string, pm25: unknown, at: string): Row => ({ id, type: "AirQualityObserved", stationName: id.toUpperCase(), pm25: pm25 as number, dateObserved: at });

describe("the monitor", () => {
  it("reads the last hour oldest first and skips points without a number", () => {
    const history = seed(
      [
        {
          id: "a",
          type: "AirQualityObserved",
          series: {
            pm25: [
              { value: 9, observedAt: "2026-09-25T10:05:00Z" },
              { value: "n/a", observedAt: "2026-09-25T10:10:00Z" },
              { value: 8, observedAt: "2026-09-25T10:00:00Z" },
            ],
          },
        },
        { id: "b", type: "AirQualityObserved", series: {} },
      ],
      "pm25",
    );
    expect(history).toEqual({ a: [{ at: "2026-09-25T10:00:00Z", value: 8 }, { at: "2026-09-25T10:05:00Z", value: 9 }] });
  });

  it("adds a poll's reading only when it is newer, keeps the last points, and returns the same history when nothing is new", () => {
    const first = record({}, [row("a", 8, "2026-09-25T10:00:00Z"), row("b", null, "2026-09-25T10:00:00Z")], "pm25", 2);
    expect(first).toEqual({ a: [{ at: "2026-09-25T10:00:00Z", value: 8 }] });
    expect(record(first, [row("a", 8, "2026-09-25T10:00:00Z")], "pm25", 2)).toBe(first);
    const third = record(record(first, [row("a", 9, "2026-09-25T10:05:00Z")], "pm25", 2), [row("a", 10, "2026-09-25T10:10:00Z")], "pm25", 2);
    expect(third.a.map((point) => point.value)).toEqual([9, 10]);
  });

  it("puts the read history under what the polls already added, without repeating a point", () => {
    const older = { a: [{ at: "10:00", value: 1 }, { at: "10:05", value: 2 }] };
    const newer = { a: [{ at: "10:05", value: 2 }, { at: "10:10", value: 3 }], b: [{ at: "10:10", value: 7 }] };
    expect(merge(older, newer)).toEqual({ a: [...older.a, { at: "10:10", value: 3 }], b: newer.b });
  });

  it("announces a crossing once, when a station goes over, and nothing on the first poll", () => {
    const calm = [row("a", 20, "t1"), row("b", 30, "t1")];
    expect(crossings(null, calm, "pm25")).toEqual([]);
    const next = [row("a", ALERT_LEVEL + 1, "t2"), row("b", 31, "t2"), row("c", 40, "t2")];
    expect(crossings(calm, next, "pm25").map((alert) => alert.station)).toEqual(["A", "C"]);
    expect(crossings(next, next, "pm25")).toEqual([]);
    expect(crossings(calm, [row("a", ALERT_LEVEL, "t2")], "pm25")).toEqual([]);
  });

  it("calls a station silent after fifteen minutes without a reading, in whole minutes", () => {
    const now = new Date("2026-09-25T12:00:00Z");
    expect(silentFor(row("a", 1, "2026-09-25T11:50:00Z"), now)).toBeNull();
    expect(silentFor(row("a", 1, "2026-09-25T11:20:30Z"), now)).toBe(39);
    expect(silentFor(row("a", 1, "not a date"), now)).toBeNull();
  });

  it("scales to the series' own range, draws a flat one along the bottom, and needs two points", () => {
    expect(sparkline([{ at: "1", value: 5 }], 100, 10)).toBe("");
    expect(sparkline([{ at: "1", value: 0 }, { at: "2", value: 10 }], 100, 10)).toBe("0,10 100,0");
    expect(sparkline([{ at: "1", value: 4 }, { at: "2", value: 4 }], 100, 10)).toBe("0,10 100,10");
  });
});
