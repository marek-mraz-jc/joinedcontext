// T-7706: vegaLiteSource.ts edges: temporal rows from malformed bodies, a source that names nothing readable, a refused CSV.
import { describe, expect, it, vi } from "vitest";
import { readSource, temporalRows } from "../src/components/dashboards/vegaLiteSource";

describe("temporalRows over malformed bodies", () => {
  it("skips entities without a string id, attribute values that are no objects, and aggregates that are no lists", () => {
    const body = [
      null,
      { id: 7, pm10: [{ values: [[1, "t"]] }] },
      { id: "urn:a", pm10: [null, "x", { avg: [[4.5, "2026-10-01T00:00:00Z"]], note: "no list" }] },
      { id: "urn:b", pm10: 5 },
    ];
    expect(temporalRows(body, ["pm10"], true)).toEqual([
      { id: "urn:a", attribute: "pm10", method: "avg", value: 4.5, observedAt: "2026-10-01T00:00:00Z" },
    ]);
    expect(temporalRows({ not: "a list" }, ["pm10"], false)).toEqual([]);
  });
});

describe("readSource refusals", () => {
  it("throws for a source whose name cannot be read", async () => {
    const fetcher = vi.fn();
    await expect(readSource("air-ops", { source: "csv", type: "not a name" }, fetcher as unknown as typeof fetch)).rejects.toThrow(/no readable source/);
    expect(fetcher).not.toHaveBeenCalled();
  });

  it("throws the status when the CSV is refused", async () => {
    const fetcher = vi.fn(async () => new Response("no", { status: 403 }));
    await expect(readSource("air-ops", { source: "csv", type: "Air" }, fetcher as unknown as typeof fetch)).rejects.toThrow("HTTP 403");
  });
});
