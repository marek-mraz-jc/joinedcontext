/**
 * Each panel type the dashboard page draws, over frames in daiya's shape: the Vega-Lite spec of a
 * chart compiled and run headless to SVG (axis labels in the field's unit, a mark per datum), the
 * DOM of the panels drawn without Vega (stat, bargauge, table), and every state of a panel card.
 */
import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { TopLevelSpec } from "vega-lite";
import "../src/i18n";
import { loadVega } from "../src/components/panels/VegaChart";
import type { Spec } from "../src/components/panels/specs";
import { barChartSpec, gaugeSpec, heatmapSpec, histogramSpec, pieChartSpec, stateTimelineSpec, timeSeriesSpec } from "../src/components/panels/specs";
import { FALLBACK } from "../src/components/panels/palette";
import { PanelView } from "../src/components/panels/PanelView";
import type { PanelDefinition } from "../src/components/panels/PanelView";
import { BarGaugePanel, shownValues, StatPanel } from "../src/components/panels/ValuePanels";
import { tableOf, TablePanel } from "../src/components/panels/TablePanel";
import { Legend } from "../src/components/panels/TimeSeriesPanel";
import { displayValue, formatDate, formatUnit, settingsFor } from "../src/components/panels/fieldConfig";
import { ordered, sections } from "../src/components/panels/DashboardGrid";
import { ColourKey } from "../src/components/panels/ChartPanels";
import { Fill, Swatch } from "../src/components/panels/paint";
import { panelQuery } from "../src/pages/grafana/DashboardPage";
import { resolveTime } from "../src/components/panels/timeRange";
import * as frames from "./fixtures/panel_frames";

const theme = { ...FALLBACK, font: "sans-serif" };

/** The chart as SVG text, drawn headless the way the page draws it (interpreter, no loads). */
async function svgOf(spec: Spec): Promise<string> {
  const [vega, vegaLite, interpreter] = await loadVega();
  const compiled = vegaLite.compile({ ...spec, width: 600 } as unknown as TopLevelSpec).spec;
  const view = new vega.View(vega.parse(compiled, undefined, { ast: true }), { expr: interpreter.expressionInterpreter, renderer: "none" });
  const svg = await view.toSVG();
  view.finalize();
  return svg;
}

function count(text: string, pattern: RegExp): number {
  return [...text.matchAll(new RegExp(pattern.source, "g"))].length;
}

describe("timeseries", () => {
  it("draws one line per label set of the context-broker request rate, its axis in req/s", async () => {
    const built = timeSeriesSpec(frames.requestRate, {
      fieldConfig: { defaults: { unit: "reqps", decimals: 1 }, overrides: [] },
      options: { legend: { displayMode: "list", placement: "bottom" } },
      locale: "en",
      theme,
      height: 200,
      from: frames.FROM,
      to: frames.TO,
    });
    expect(built.series.map((one) => one.name)).toEqual(["200", "500"]);
    const svg = await svgOf(built.spec);
    expect(count(svg, /class="mark-line/)).toBeGreaterThanOrEqual(1);
    expect(svg).toMatch(/\d+(\.\d)? req\/s/);
  });

  it("spans the asked time range on its x axis, not years", async () => {
    const built = timeSeriesSpec(frames.requestRate, { options: {}, locale: "en", theme, height: 200, from: frames.FROM, to: frames.TO });
    const [vega, vegaLite, interpreter] = await loadVega();
    const compiled = vegaLite.compile({ ...built.spec, width: 600 } as unknown as TopLevelSpec).spec;
    const view = new vega.View(vega.parse(compiled, undefined, { ast: true }), { expr: interpreter.expressionInterpreter, renderer: "none" });
    await view.runAsync();
    expect((view.scale("x").domain() as Date[]).map((at) => at.getTime())).toEqual([frames.FROM, frames.TO]);
    view.finalize();
  });

  it("draws the helsinki pm25 history with a threshold line and a table legend of its calcs", async () => {
    const fieldConfig = {
      defaults: { unit: "suffix: µg/m³", thresholds: { mode: "absolute", steps: [{ color: "green", value: null }, { color: "red", value: 15 }] }, custom: { thresholdsStyle: { mode: "line" } } },
      overrides: [],
    };
    const built = timeSeriesSpec(frames.pm25History, { fieldConfig, options: {}, locale: "en", theme, height: 200, from: frames.FROM, to: frames.TO });
    const svg = await svgOf(built.spec);
    expect(svg).toContain("µg/m³");
    expect(svg).toMatch(/class="mark-rule/);
    render(
      <Legend
        series={built.series}
        colors={built.colors}
        legend={{ displayMode: "table", placement: "bottom", calcs: ["mean", "max"] }}
        fieldConfig={fieldConfig}
        locale="en"
        hidden={new Set()}
        onToggle={() => undefined}
      />,
    );
    const table = screen.getByRole("table");
    expect(within(table).getByRole("columnheader", { name: "Mean" })).toBeInTheDocument();
    expect(within(table).getByRole("row", { name: /Mannerheimintie.*16\.1 µg\/m³/ })).toBeInTheDocument();
  });

  it("hides a series its legend entry turns off", async () => {
    const user = userEvent.setup();
    const toggled: string[] = [];
    const built = timeSeriesSpec(frames.requestRate, { options: {}, locale: "en", theme, height: 200, from: frames.FROM, to: frames.TO });
    render(<Legend series={built.series} colors={built.colors} legend={{ displayMode: "list" }} locale="en" hidden={new Set()} onToggle={(name) => toggled.push(name)} />);
    await user.click(screen.getByRole("button", { name: /500/ }));
    expect(toggled).toEqual(["500"]);
  });

  it("heads its calcs with Grafana's reducer names and sorts by the name Grafana saved", () => {
    const built = timeSeriesSpec(frames.requestRate, { options: {}, locale: "en", theme, height: 200, from: frames.FROM, to: frames.TO });
    const { unmount } = render(
      <Legend series={built.series} colors={built.colors} legend={{ displayMode: "table", calcs: ["lastNotNull", "current", "p95", "delta"], sortBy: "Last *", sortDesc: true }} locale="en" hidden={new Set()} onToggle={() => undefined} />,
    );
    const heads = screen.getAllByRole("columnheader").map((head) => head.textContent);
    expect(heads.slice(1)).toEqual(["Last *", "Last *", "95th %", "Delta"]);
    const lasts = screen.getAllByRole("row").slice(1).map((row) => Number(within(row).getAllByRole("cell")[1].textContent?.replace(/[^\d.]/g, "")));
    expect(lasts).toEqual([...lasts].sort((a, b) => b - a));
    unmount();
    render(<Legend series={built.series} colors={built.colors} legend={{ displayMode: "list", calcs: ["mean"] }} locale="en" hidden={new Set()} onToggle={() => undefined} />);
    expect(screen.getAllByText(/^Mean: /).length).toBe(built.series.length);
  });
});

describe("table", () => {
  const fieldConfig = {
    defaults: {},
    overrides: [
      { matcher: { id: "byName", options: "pm25" }, properties: [{ id: "unit", value: "suffix: µg/m³" }, { id: "decimals", value: 1 }, { id: "custom.width", value: 120 }] },
      {
        matcher: { id: "byName", options: "airQualityLevel" },
        properties: [
          { id: "displayName", value: "Level" },
          { id: "mappings", value: [{ type: "value", options: { poor: { text: "Poor", color: "red" } } }] },
        ],
      },
    ],
  };

  it("names its columns by the overrides and gives the grid one entity per row", () => {
    const table = tableOf(frames.stations, fieldConfig);
    expect(table.columns.map((column) => column.label)).toEqual(["name", "pm25", "Level", "dateObserved"]);
    expect(table.entities).toHaveLength(3);
    expect(table.columns[1].settings.custom.width).toBe(120);
  });

  it("draws each frame as its own table, picked below the grid as Grafana's frame select does", async () => {
    const user = userEvent.setup();
    const host = (name: string, refId: string, value: number) => ({ refId, fields: [{ name: "host", type: "string" as const, values: [name] }, { name: "Value", type: "number" as const, values: [value] }] });
    render(<TablePanel title="Days left" frames={[host("dev", "A", 7), host("api", "B", 8)]} locale="en" />);
    expect(await screen.findByText("dev", {}, { timeout: 5000 })).toBeInTheDocument();
    expect(screen.queryByText("api")).toBeNull();
    const select = screen.getByRole("combobox", { name: "Data frame of Days left" });
    expect(within(select).getAllByRole("option").map((option) => option.textContent)).toEqual(["A", "B"]);
    await user.selectOptions(select, "1");
    expect(await screen.findByText("api", {}, { timeout: 5000 })).toBeInTheDocument();
    expect(screen.queryByText("dev")).toBeNull();
  });

  it("draws the helsinki stations in the SDK grid, each cell in its unit and mapping", async () => {
    render(<TablePanel title="Stations" frames={frames.stations} fieldConfig={fieldConfig} locale="en" />);
    expect(await screen.findByText("26.3 µg/m³", {}, { timeout: 5000 })).toBeInTheDocument();
    expect(screen.getByText("Poor")).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: /Level/ })).toBeInTheDocument();
  });
});

describe("stat", () => {
  it("shows each instance's last value, coloured by its threshold, named when there are several", () => {
    const fieldConfig = { defaults: { thresholds: { mode: "absolute", steps: [{ color: "red", value: null }, { color: "green", value: 1 }] } }, overrides: [] };
    const shown = shownValues(frames.up, fieldConfig, { reduceOptions: { calcs: ["lastNotNull"] } }, "en");
    expect(shown.map((one) => [one.reduced.name, one.text])).toEqual([
      ["antares-0", "1"],
      ["antares-1", "0"],
    ]);
    expect(shown[0].color).not.toBe(shown[1].color);
    render(<StatPanel title="Up" frames={frames.up} fieldConfig={fieldConfig} options={{ reduceOptions: { calcs: ["lastNotNull"] } }} height={120} locale="en" />);
    expect(screen.getByText("antares-1")).toBeInTheDocument();
    expect(screen.getAllByRole("img", { name: /Trend of/ })).toHaveLength(2);
  });
});

describe("gauge", () => {
  it("draws the value's arc and prints it in the middle", async () => {
    const svg = await svgOf(gaugeSpec(812e6, formatUnit(812e6, "bytes", 0, "en"), { min: 0, max: 1e9, thresholds: { mode: "absolute", steps: [{ color: "green", value: null }, { color: "red", value: 8e8 }] } }, { theme, height: 160, showThresholdMarkers: true, minText: "0 B", maxText: "954 MiB" }));
    expect(count(svg, /class="mark-arc/)).toBeGreaterThanOrEqual(1);
    expect(svg).toContain("774 MiB");
    // The scale's ends, under the arc's ends.
    expect(svg).toContain(">0 B<");
    expect(svg).toContain(">954 MiB<");
  });
});

describe("bargauge", () => {
  it("gives each pod a meter as long as its share, labelled in bytes", () => {
    render(<BarGaugePanel title="Memory" frames={frames.memory} fieldConfig={{ defaults: { unit: "bytes", min: 0, max: 1e9 }, overrides: [] }} options={{}} height={120} locale="en" />);
    const meter = screen.getByRole("meter", { name: "portal" });
    expect(meter).toHaveAttribute("aria-valuenow", "812000000");
    expect(meter).toHaveAttribute("aria-valuetext", "774 MiB");
  });

  it("says what its colours mean: each threshold and the value it starts at", () => {
    const thresholds = { mode: "absolute", steps: [{ color: "green", value: null }, { color: "red", value: 750e6 }] };
    render(<BarGaugePanel title="Memory" frames={frames.memory} fieldConfig={{ defaults: { unit: "bytes", thresholds }, overrides: [] }} options={{}} height={120} locale="en" />);
    const key = screen.getByRole("list", { name: "Colours by value" });
    expect(within(key).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["below 715 MiB", "from 715 MiB"]);
  });
});

describe("barchart and piechart", () => {
  it("draws one bar per route", async () => {
    const built = barChartSpec(frames.routes, { options: {}, locale: "en", theme, height: 200 });
    const svg = await svgOf(built.spec);
    expect(count(svg, /<path [^>]*role="graphics-symbol"/)).toBeGreaterThanOrEqual(3);
    expect(svg).toContain("/q/health");
  });

  it("draws one slice per route and keeps their values for the key", async () => {
    const built = pieChartSpec(frames.routes, { options: { reduceOptions: { values: true } }, locale: "en", theme, height: 200 });
    expect(built.slices.map((slice) => slice.value)).toEqual([5120, 2400, 830]);
    const svg = await svgOf(built.spec);
    expect(count(svg, /class="mark-arc/)).toBeGreaterThanOrEqual(1);
  });
});

describe("colour keys", () => {
  it("names each colour of a chart beside its swatch, a share after it", () => {
    render(<ColourKey label="Legend" items={[{ name: "entities", color: "#0f766e", detail: "59.1%" }, { name: "health", color: "#d97706" }]} />);
    const key = screen.getByRole("list", { name: "Legend" });
    expect(within(key).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["entities59.1%", "health"]);
  });

  it("paints a swatch and a bar fill in the dashboard's colour, the fill as long as its share", () => {
    const { container } = render(
      <div>
        <Swatch color="#dc2626" />
        <Fill share={0.25} color="#15803d" />
      </div>,
    );
    const [swatch, fill] = [...container.querySelectorAll("span")];
    expect(swatch).toHaveStyle({ backgroundColor: "#dc2626" });
    expect(fill).toHaveStyle({ width: "25%" });
  });

  it("draws nothing for a chart with no colours to name", () => {
    const { container } = render(<ColourKey label="Legend" items={[]} />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("heatmap", () => {
  it("draws the duration buckets as rows, smallest at the bottom", async () => {
    const built = heatmapSpec(frames.durationBuckets, { options: {}, locale: "en", theme, height: 200, from: frames.FROM, to: frames.TO });
    expect(built.buckets).toEqual(["0.05", "0.1", "0.5", "1", "+Inf"]);
    const svg = await svgOf(built.spec);
    expect(count(svg, /<path [^>]*role="graphics-symbol"/)).toBe(20);
  });
});

describe("state-timeline", () => {
  it("draws each state from its time to the next, one colour per state", async () => {
    const built = stateTimelineSpec(frames.runState, {
      fieldConfig: { defaults: { mappings: [{ type: "value", options: { failing: { text: "Failing", color: "red" } } }] }, overrides: [] },
      options: {},
      locale: "en",
      theme,
      height: 100,
      from: frames.FROM,
      to: frames.TO,
    });
    expect(built.states.map((state) => state.text)).toEqual(["ok", "Failing", "running"]);
    const svg = await svgOf(built.spec);
    expect(count(svg, /<path [^>]*role="graphics-symbol"/)).toBe(4);
  });
});

describe("histogram", () => {
  it("bins every value of the frames", async () => {
    const svg = await svgOf(histogramSpec(frames.pm25History, { options: { bucketCount: 5 }, locale: "en", theme, height: 160 }));
    expect(count(svg, /<path [^>]*role="graphics-symbol"/)).toBeGreaterThanOrEqual(2);
  });
});

describe("a panel card", () => {
  const panel: PanelDefinition = { id: 2, type: "stat", title: "Up", gridPos: { x: 0, y: 0, w: 6, h: 4 } };
  const common = { from: frames.FROM, to: frames.TO, locale: "en", height: 120 };

  it("announces its read while it loads", () => {
    render(<PanelView panel={panel} loading {...common} />);
    expect(screen.getByRole("status", { name: "Reading Up" })).toBeInTheDocument();
  });

  it("says a range with no readings is empty, in the panel's own noValue when it has one", () => {
    render(<PanelView panel={{ ...panel, fieldConfig: { defaults: { noValue: "No readings" }, overrides: [] } }} data={{ frames: frames.empty }} {...common} />);
    expect(screen.getByText("No readings")).toBeInTheDocument();
  });

  it("names the datasource's failure and offers a retry", async () => {
    const user = userEvent.setup();
    let retried = 0;
    render(<PanelView panel={panel} data={{ frames: [], error: "prometheus answered 502" }} onRetry={() => (retried += 1)} {...common} />);
    expect(screen.getByRole("alert")).toHaveTextContent("The data source did not answer");
    // The service's own words stay folded under Details: an operator's, not a next step.
    expect(screen.getByText("prometheus answered 502").closest("details")).not.toHaveAttribute("open");
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(retried).toBe(1);
  });

  it("names the range it read when empty and offers a longer one", async () => {
    const user = userEvent.setup();
    let widened = 0;
    render(<PanelView panel={panel} data={{ frames: frames.empty }} asked={{ range: "Last hour", vars: "Station Kallio" }} onWiden={() => (widened += 1)} {...common} />);
    expect(screen.getByText("No readings in “Last hour” for Station Kallio. Show a longer range or pick other values.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show a longer range" }));
    expect(widened).toBe(1);
  });

  it("says every reading lies outside the range and zooms to the data, as Grafana's time series does", async () => {
    const user = userEvent.setup();
    const zoomed: number[][] = [];
    const series: PanelDefinition = { id: 3, type: "timeseries", title: "Rate" };
    const before = [{ fields: [{ name: "Time", type: "time" as const, values: [frames.FROM - 7_200_000, frames.FROM - 3_600_000] }, { name: "Value", type: "number" as const, values: [1, 2] }] }];
    render(<PanelView panel={series} data={{ frames: before }} onZoom={(from, to) => zoomed.push([from, to])} {...common} />);
    expect(screen.getByRole("status")).toHaveTextContent("Data outside time range");
    await user.click(screen.getByRole("button", { name: "Zoom to data" }));
    expect(zoomed).toEqual([[frames.FROM - 7_200_000, frames.FROM - 3_600_000]]);
  });

  it("keeps a failed panel to one line when the page already sums up the failures", () => {
    render(<PanelView panel={panel} data={{ frames: [], error: "prometheus answered 502" }} onRetry={() => undefined} quiet {...common} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Not read: see the notice above the panels.");
    expect(screen.queryByRole("button", { name: "Try again" })).not.toBeInTheDocument();
  });

  it("says why a source the person may not read shows nothing", () => {
    render(<PanelView panel={panel} data={{ frames: [], accessDenied: "You cannot read the source of this panel: helsinki-air" }} {...common} />);
    expect(screen.getByText("You cannot read this panel's data")).toBeInTheDocument();
    expect(screen.getByText("You cannot read the source of this panel: helsinki-air. Ask a project owner for read access to it.")).toBeInTheDocument();
  });

  it("draws a type it does not know as a table and says so", async () => {
    render(<PanelView panel={{ ...panel, type: "geomap" }} data={{ frames: frames.stations }} {...common} />);
    expect(screen.getByText("geomap, shown as a table")).toBeInTheDocument();
    expect(await screen.findByText("Vartiokylä", {}, { timeout: 5000 })).toBeInTheDocument();
  });

  it("shows a text panel's words without markup", () => {
    render(<PanelView panel={{ ...panel, type: "text", options: { content: "## About\n**Air** quality" } }} {...common} />);
    expect(screen.getByText(/About\s+Air quality/)).toBeInTheDocument();
  });
});

describe("the dashboard around the panels", () => {
  it("orders panels by row then column, and folds each row's own panels under it", () => {
    const panels: PanelDefinition[] = [
      { id: 3, type: "stat", gridPos: { x: 12, y: 0, w: 12, h: 4 } },
      { id: 1, type: "stat", gridPos: { x: 0, y: 0, w: 12, h: 4 } },
      { id: 9, type: "row", title: "Details", collapsed: true, gridPos: { x: 0, y: 4, w: 24, h: 1 }, panels: [{ id: 10, type: "table", gridPos: { x: 0, y: 5, w: 24, h: 8 } }] },
    ];
    expect(ordered(panels).map((panel) => panel.id)).toEqual([1, 3, 9]);
    expect(sections(panels).map((section) => [section.row?.id, section.panels.map((panel) => panel.id)])).toEqual([
      [undefined, [1, 3]],
      [9, [10]],
    ]);
  });

  it("asks a panel for the range and each variable, and nothing else", () => {
    expect(panelQuery({ from: 1000, to: 2000 }, { station: ["Kallio"], testid: ["$__all"] }, 500)).toBe("from=1000&to=2000&maxDataPoints=500&var-station=Kallio&var-testid=%24__all");
  });

  it("reads Grafana's time words", () => {
    const now = Date.UTC(2026, 10, 8, 12, 0, 0);
    expect(resolveTime("now-6h", now)).toBe(now - 6 * 3_600_000);
    expect(resolveTime("1700000000000", now)).toBe(1_700_000_000_000);
    expect(resolveTime("yesterday", now)).toBeNull();
  });

  it("shows a time field as a date whatever unit the panel set, with milliseconds inside a minute, as Grafana does", () => {
    const at = new Date(2026, 10, 8, 7, 0, 0).getTime();
    const one = settingsFor({ defaults: { unit: "percentunit" } }, { name: "Time", type: "time", values: [at, at] });
    expect(one.unit).toBe("dateTimeAsSystem");
    expect(displayValue(one, at).text).toBe("2026-11-08 07:00:00.000");
    const hourly = settingsFor({ defaults: { unit: "d" } }, { name: "Time", type: "time", values: [at, at + 3_600_000] });
    expect(displayValue(hourly, at).text).toBe("2026-11-08 07:00:00");
    // A date unit the panel names stays, and a number in it reads as a date.
    const iso = settingsFor({ defaults: { unit: "dateTimeAsIso" } }, { name: "observedAt", type: "number", values: [at] });
    expect(displayValue(iso, at).text).toBe("2026-11-08 07:00:00");
    expect(displayValue(iso, new Date(at).toISOString()).text).toBe("2026-11-08 07:00:00");
    expect(formatDate(at, "dateTimeAsUS")).toBe("11/08/2026 7:00:00 am");
    expect(formatDate(at, "dateTimeAsIsoNoDateIfToday", false, "en", at + 60_000)).toBe("07:00:00");
    expect(formatDate(at, "time:YYYY/MM/DD")).toBe("2026/11/08");
    expect(formatDate(at - 3 * 3_600_000, "dateTimeFromNow", false, "en", at)).toBe("3 hours ago");
  });

  it("keeps an override's unit for its field only", () => {
    const config = { defaults: { unit: "percent" }, overrides: [{ matcher: { id: "byName", options: "pm25" }, properties: [{ id: "unit", value: "bytes" }] }] };
    expect(settingsFor(config, { name: "pm25", type: "number", values: [] }).unit).toBe("bytes");
    expect(settingsFor(config, { name: "no2", type: "number", values: [] }).unit).toBe("percent");
  });
});
