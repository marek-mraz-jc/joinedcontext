import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import { dailyMeans, GUIDELINE, readings, stationMeans, summarise } from "./story";

const reading = (station: string, day: string, pm25: unknown): Row => ({
  id: `urn:ngsi-ld:AirQualityObserved:hel.fi:air:${station}-${day}`,
  type: "AirQualityObserved",
  stationName: station,
  pm25: pm25 as number,
  dateObserved: `${day}T12:00:00Z`,
});

describe("the story's numbers", () => {
  it("leaves out a reading without a station, a date or a finite value", () => {
    const list = readings([
      reading("A", "2026-09-01", 10),
      reading("A", "2026-09-02", Number.NaN),
      reading("A", "2026-09-03", "12"),
      { ...reading("A", "2026-09-04", 9), stationName: null },
      { ...reading("A", "2026-09-05", 9), dateObserved: "yesterday" },
    ]);
    expect(list).toEqual([{ station: "A", day: "2026-09-01", pm25: 10 }]);
  });

  it("means each day over the stations that reported it, and each station over its days", () => {
    const list = readings([reading("A", "2026-09-01", 20), reading("B", "2026-09-01", 10), reading("A", "2026-09-02", 12)]);
    expect(dailyMeans(list)).toEqual([
      { day: "2026-09-01", mean: 15 },
      { day: "2026-09-02", mean: 12 },
    ]);
    expect(stationMeans(list)).toEqual([
      { station: "A", mean: 16, daysOver: 1 },
      { station: "B", mean: 10, daysOver: 0 },
    ]);
  });

  it("counts a day at exactly the guideline as not over it", () => {
    const story = summarise(readings([reading("A", "2026-09-01", GUIDELINE), reading("A", "2026-09-02", GUIDELINE + 0.1)]));
    expect(story?.daysOver).toBe(1);
  });

  it("tells a week's change only with two weeks of data, and nothing without data", () => {
    const days = (count: number, from: number, step: number) =>
      Array.from({ length: count }, (_, i) => reading("A", `2026-09-${String(i + 1).padStart(2, "0")}`, from + step * Math.floor(i / 7)));
    expect(summarise(readings(days(13, 10, 2)))?.change).toBeNull();
    expect(summarise(readings(days(14, 10, 2)))?.change).toBeCloseTo(2);
    expect(summarise([])).toBeNull();
  });
});
