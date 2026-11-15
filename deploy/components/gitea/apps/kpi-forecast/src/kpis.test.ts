import { describe, expect, it } from "vitest";
import type { Row, TemporalRow } from "@joinedcontext/sdk";
import { EMPTY, numberOf, readView, shortId, toSeries, writeView } from "./kpis";

describe("the view in the address", () => {
  it("reads what it names and falls back on anything else", () => {
    expect(readView("")).toEqual(EMPTY);
    expect(readView("?kpi=urn%3Ax%3A1&days=7&odd=1&lang=fi")).toEqual({ kpi: "urn:x:1", days: 7, odd: true });
    expect(readView("?days=45&odd=yes")).toEqual(EMPTY);
  });

  it("writes only what differs from the default and keeps the language", () => {
    expect(writeView("?lang=en", EMPTY)).toBe("?lang=en");
    expect(writeView("?lang=en&days=7", { kpi: "urn:x:1", days: 90, odd: true })).toBe("?lang=en&kpi=urn%3Ax%3A1&days=90&odd=1");
    expect(writeView("", EMPTY)).toBe("");
  });
});

describe("the series handed to the module", () => {
  const row = (id: string, currentValue: unknown) => ({ id, type: "KeyPerformanceIndicator", currentValue }) as Row;
  const history = (id: string, points: [unknown, string][]): TemporalRow => ({
    id,
    type: "KeyPerformanceIndicator",
    series: { currentValue: points.map(([value, observedAt]) => ({ value: value as number, observedAt })) },
  });

  it("takes each indicator's history, drops what is not a number or a time, and never invents a point", () => {
    const series = toSeries(
      [row("a", 5), row("b", 9)],
      [history("a", [[1, "2030-10-01T00:00:00Z"], ["2", "2030-10-01T01:00:00Z"], ["x", "2030-10-01T02:00:00Z"], [3, "not a time"]])],
    );
    expect(series).toEqual([
      { id: "a", points: [{ t: Date.UTC(2030, 9, 1), v: 1 }, { t: Date.UTC(2030, 9, 1, 1), v: 2 }] },
      { id: "b", points: [] },
    ]);
  });

  it("reads numbers and numeric texts only", () => {
    expect([numberOf(3), numberOf("4.5"), numberOf(""), numberOf("x"), numberOf(null), numberOf(Number.NaN)]).toEqual([3, 4.5, null, null, null, null]);
  });

  it("names an indicator by the last segment of its id", () => {
    expect(shortId("urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:bike-network-capacity")).toBe("bike-network-capacity");
    expect(shortId("plain")).toBe("plain");
  });
});
