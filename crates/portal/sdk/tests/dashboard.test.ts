import { describe, expect, it } from "vitest";

import { chartBars, filtersQ, kpiValue, parseDashboardConfig, toneOf } from "../src/grid/dashboard";
import type { RichRow } from "../src/grid/model";
import cases from "./fixtures/dashboard_cases.json";

const row = (id: string, cells: Record<string, unknown>): RichRow => ({
  id,
  type: "BikeHireDockingStation",
  cells: Object.fromEntries(Object.entries(cells).map(([k, value]) => [k, { kind: "property" as const, value }])),
  raw: {},
});

describe("parseDashboardConfig (T-6402)", () => {
  // The Portal judges the same files in src/api/app_dashboard.rs: both refuse at the same paths.
  for (const each of cases.cases) {
    it(`${each.name}: ${each.refused.length === 0 ? "accepted" : `refused at ${each.refused.join(", ")}`}`, () => {
      const { config, findings } = parseDashboardConfig(each.config, cases.known);
      const paths = findings.map((f) => f.path);
      for (const path of each.refused) expect(paths).toContain(path);
      if (each.refused.length === 0) {
        expect(findings).toEqual([]);
        expect(config?.grid.type).toBe("BikeHireDockingStation");
      } else expect(config).toBeUndefined();
    });
  }
});

describe("what a dashboard draws", () => {
  const rows = [
    row("a", { name: "Kamppi", availableBikeNumber: 0, occupancy: 0 }),
    row("b", { name: "Töölö", availableBikeNumber: 4, occupancy: 20 }),
    row("c", { name: "Kallio", availableBikeNumber: 12, occupancy: 80 }),
    row("d", { name: "Pasila" }),
  ];
  const colour = { attr: "occupancy", bands: [{ below: 10, tone: "danger" as const, label: "Almost empty" }, { below: 50, tone: "warning" as const, label: "Running low" }, { tone: "success" as const, label: "Bikes available" }] };

  it("tones a row by the first band its value is under, and leaves a row without a number alone", () => {
    expect(rows.map((r) => toneOf(colour, r)?.tone)).toEqual(["danger", "warning", "success", undefined]);
  });

  it("counts the rows a where formula makes true and adds up an attribute", () => {
    expect(kpiValue({ label: "Stations", agg: "count" }, rows)).toBe(4);
    expect(kpiValue({ label: "Empty", agg: "count", where: "availableBikeNumber == 0" }, rows)).toBe(1);
    expect(kpiValue({ label: "Bikes", agg: "sum", attr: "availableBikeNumber" }, rows)).toBe(16);
    expect(kpiValue({ label: "Least", agg: "min", attr: "missing" }, rows)).toBeNull();
  });

  it("charts one bar per value of x, largest first", () => {
    expect(chartBars({ x: "name", y: "availableBikeNumber", agg: "sum" }, rows)).toEqual([
      { x: "Kallio", y: 12 },
      { x: "Töölö", y: 4 },
      { x: "Kamppi", y: 0 },
    ]);
  });

  it("turns the filters into the Endpoint's q, text in quotes", () => {
    expect(filtersQ([{ attr: "status", op: "==", value: "working" }, { attr: "totalSlotNumber", op: ">", value: 0 }])).toBe('status=="working";totalSlotNumber>0');
    expect(filtersQ([])).toBeUndefined();
  });
});
