// The panels' small rules: times and names of frame fields, the null modes of the reducers, the
// rounding units of a range, the draw styles of a time series and the table's sort and alignment.
import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import "../src/i18n";
import { displayNameOf, numberOf, rowsOf, timeOf } from "../src/components/panels/frames";
import { calcValueOf, reduceField } from "../src/components/panels/reducers";
import { resolveTime } from "../src/components/panels/timeRange";
import { timeSeriesSpec } from "../src/components/panels/specs";
import { chartTheme } from "../src/components/panels/VegaChart";
import { TableCellValue, TablePanel, tableOf } from "../src/components/panels/TablePanel";
import { markersOf } from "../src/components/panels/GeomapPanel";
import { DashboardGrid } from "../src/components/panels/DashboardGrid";
import userEvent from "@testing-library/user-event";
import i18n from "../src/i18n";
import * as frames from "./fixtures/panel_frames";

describe("frames", () => {
  it("reads a time from a number or ISO text, nothing from other values", () => {
    expect(timeOf("2026-11-08T06:00:00Z")).toBe(Date.UTC(2026, 10, 8, 6));
    expect(timeOf("yesterday-ish")).toBeNull();
    expect(timeOf({})).toBeNull();
  });

  it("reads a number from numeric text, nothing from other text", () => {
    expect(numberOf(" 4.5 ")).toBe(4.5);
    expect(numberOf("four")).toBeNull();
  });

  it("names a field of several labels by all of them", () => {
    expect(displayNameOf({ name: "Value", type: "number", values: [], labels: { job: "edge", code: "200", __name__: "x" } })).toBe("job=edge, code=200");
  });

  it("numbers a second field of the same name in one frame", () => {
    const table = rowsOf([{ name: "", fields: [{ name: "x", type: "number", values: [1] }, { name: "x", type: "number", values: [2] }] }]);
    expect(table.names).toEqual(["x", "x 2"]);
    expect(table.rows[0]).toMatchObject({ x: 1, "x 2": 2 });
  });
});

describe("reducers in the null-as-zero mode", () => {
  const field = { values: [4, null, 2], numeric: true, nullValueMode: "null as zero" as const };
  it("counts a null as zero for a percentile, the median and the standard calcs", () => {
    expect(reduceField(field, ["p50"])).toEqual({ p50: 2 });
    expect(reduceField(field, ["median"])).toEqual({ median: 2 });
    expect(reduceField(field, ["min", "mean"])).toMatchObject({ min: 0, mean: 2 });
  });

  it("reads Prometheus' infinity texts", () => {
    expect(calcValueOf("+Inf")).toBe(Number.POSITIVE_INFINITY);
    expect(calcValueOf(" Inf ")).toBe(Number.POSITIVE_INFINITY);
    expect(calcValueOf("-Inf")).toBe(Number.NEGATIVE_INFINITY);
    expect(calcValueOf(" 4 ")).toBe(4);
    expect(calcValueOf("")).toBeNull();
    expect(calcValueOf("four")).toBeNull();
  });
});

describe("resolveTime rounding", () => {
  const now = new Date(2026, 10, 8, 15, 42, 17, 500).getTime();
  it("rounds down to the hour, the minute and the second", () => {
    expect(resolveTime("now/h", now)).toBe(new Date(2026, 10, 8, 15).getTime());
    expect(resolveTime("now/m", now)).toBe(new Date(2026, 10, 8, 15, 42).getTime());
    expect(resolveTime("now/s", now)).toBe(new Date(2026, 10, 8, 15, 42, 17).getTime());
  });
});

describe("time series draw styles", () => {
  const spec = (custom: Record<string, unknown>) =>
    JSON.stringify(timeSeriesSpec(frames.requestRate, { fieldConfig: { defaults: { custom }, overrides: [] }, from: frames.FROM, to: frames.TO, locale: "en", theme: chartTheme("light"), height: 200 }).spec);
  it("draws bars, points and each line interpolation", () => {
    expect(spec({ drawStyle: "bars" })).toContain('"type":"bar"');
    expect(spec({ drawStyle: "points" })).toContain('"type":"point"');
    expect(spec({ fillOpacity: 20 })).toContain('"type":"area"');
    expect(spec({ lineInterpolation: "smooth" })).toContain('"monotone"');
    expect(spec({ lineInterpolation: "stepBefore" })).toContain('"step-before"');
    expect(spec({ lineInterpolation: "stepAfter" })).toContain('"step-after"');
  });
});

describe("table", () => {
  it("sorts an empty cell last after a value", () => {
    const table = tableOf([{ name: "", fields: [{ name: "n", type: "number", values: [null, 3, null, 1, 2] }] }], undefined, [{ displayName: "n" }]);
    const attr = table.columns[0].attr;
    expect(table.entities.map((row) => (row[attr] as { value: number } | undefined)?.value ?? null)).toEqual([1, 2, 3, null, null]);
  });

  it("sorts text by locale, descending where asked", () => {
    const table = tableOf([{ name: "", fields: [{ name: "s", type: "string", values: ["b", "c", "a"] }] }], undefined, [{ displayName: "s", desc: true }]);
    const attr = table.columns[0].attr;
    expect(table.entities.map((row) => (row[attr] as { value: string }).value)).toEqual(["c", "b", "a"]);
  });

  it("centres a cell where the field asks", () => {
    render(<TableCellValue settings={{ mappings: [], custom: { align: "center" } }} value="mid" locale="en" />);
    expect(screen.getByText("mid").closest(".text-center")).not.toBeNull();
  });

  it("paints a colour-background cell in its threshold colour", () => {
    const { container } = render(<TableCellValue settings={{ mappings: [], custom: { cellOptions: { type: "color-background" } } }} value={95} locale="en" />);
    expect(container.querySelector(".rounded-sm")).toHaveTextContent("95");
  });

  it("draws a field Grafana gave an empty display name and a fractional width under its own name", () => {
    const fieldConfig = { defaults: { custom: { width: 120.5 } }, overrides: [{ matcher: { id: "byName", options: "n" }, properties: [{ id: "displayName", value: "" }] }] };
    render(<TablePanel title="Counts" frames={[{ name: "", fields: [{ name: "n", type: "number", values: [1] }] }]} fieldConfig={fieldConfig} locale="en" />);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getAllByText("n").length).toBeGreaterThan(0);
  });
});

describe("geomap locator", () => {
  it("places no marker in the geojson mode when no field holds a geometry", () => {
    const plain = [{ name: "", fields: [{ name: "lat", type: "number" as const, values: [60.1] }, { name: "lon", type: "number" as const, values: [24.9] }] }];
    expect(markersOf(plain, { layers: [{ location: { mode: "geojson" } }] }).features.features).toHaveLength(0);
    expect(markersOf(plain, { layers: [{ location: { mode: "auto" } }] }).features.features).toHaveLength(1);
  });
});

describe("dashboard rows", () => {
  it("folds a row open and shut, a collapsed row starts shut and an untitled one is named", async () => {
    await i18n.changeLanguage("en");
    const panels = [
      { id: 1, type: "row", title: "Traffic", gridPos: { x: 0, y: 0, w: 24, h: 1 } },
      { id: 2, type: "stat", title: "Requests", gridPos: { x: 0, y: 1, w: 12, h: 4 } },
      { id: 3, type: "row", title: "", collapsed: true, gridPos: { x: 0, y: 5, w: 24, h: 1 }, panels: [{ id: 4, type: "stat", title: "Errors", gridPos: { x: 0, y: 6, w: 12, h: 4 } }] },
    ];
    render(<DashboardGrid panels={panels} render={(panel) => <p>{`panel ${panel.title ?? ""}`}</p>} />);
    const traffic = screen.getByRole("button", { name: "Traffic" });
    expect(traffic).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("panel Requests")).toBeInTheDocument();
    expect(screen.queryByText("panel Errors")).toBeNull();
    await userEvent.click(traffic);
    expect(screen.queryByText("panel Requests")).toBeNull();
    const untitled = screen.getByRole("button", { name: i18n.t("dashboardView.untitled") });
    expect(untitled).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(untitled);
    expect(screen.getByText("panel Errors")).toBeInTheDocument();
  });
});
