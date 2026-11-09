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
import { autoPoints, spanned, barChartSpec, gaugeSpec, heatmapSpec, histogramSpec, pieChartSpec, stateTimelineSpec, timeSeriesSpec } from "../src/components/panels/specs";
import { FALLBACK } from "../src/components/panels/palette";
import { PanelView, plainText } from "../src/components/panels/PanelView";
import type { PanelDefinition } from "../src/components/panels/PanelView";
import { barGradient, BarGaugePanel, DEFAULT_THRESHOLDS, globalRange, shownValues, StatPanel } from "../src/components/panels/ValuePanels";
import { PhoneTable, TableCellValue, tableOf, TablePanel } from "../src/components/panels/TablePanel";
import { Legend } from "../src/components/panels/TimeSeriesPanel";
import type { LegendOptions } from "../src/components/panels/TimeSeriesPanel";
import { colorOf, displayValue, formatDate, formatUnit, settingsFor } from "../src/components/panels/fieldConfig";
import type { Frame } from "../src/components/panels/frames";
import { ordered, sections } from "../src/components/panels/DashboardGrid";
import { ColourKey } from "../src/components/panels/ChartPanels";
import { Fill, Swatch } from "../src/components/panels/paint";
import { panelQuery } from "../src/pages/grafana/DashboardPage";
import { resolveTime } from "../src/components/panels/timeRange";
import * as frames from "./fixtures/panel_frames";
import builderFile from "../../tests/fixtures/grafana-builder/bike-stations.json";
import recordedPm25 from "./fixtures/frames/helsinki-2.json";
import recordedBikes from "./fixtures/frames/helsinki-4.json";

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

  it("labels time ticks in Grafana's words and draws a grid line at each, ticks a value per 30 px", () => {
    const axis = (span: number) => {
      const built = timeSeriesSpec(frames.requestRate, { options: {}, locale: "en", theme, height: 200, from: frames.TO - span, to: frames.TO });
      const layer = (built.spec as { layer?: { encoding?: { x?: { axis?: { format?: string; grid?: boolean } }; y?: { axis?: { tickCount?: number } } } }[] }).layer ?? [];
      return { x: layer.find((one) => one.encoding?.x?.axis)?.encoding?.x?.axis, y: layer.find((one) => one.encoding?.y?.axis)?.encoding?.y?.axis };
    };
    expect(axis(6 * 3_600_000).x).toMatchObject({ format: "%H:%M", grid: true });
    expect(axis(7 * 86_400_000).x).toMatchObject({ format: "%m/%d %H:%M" });
    expect(axis(30 * 86_400_000).x).toMatchObject({ format: "%m/%d" });
    expect(axis(6 * 3_600_000).y?.tickCount).toBe(6);
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

  it("keeps a bottom table legend in a scroll box on a desktop, and every row in sight on a phone", () => {
    const built = timeSeriesSpec(frames.requestRate, { options: {}, locale: "en", theme, height: 200, from: frames.FROM, to: frames.TO });
    const legend: LegendOptions = { displayMode: "table", placement: "bottom", calcs: ["max"] };
    const { container, unmount } = render(<Legend series={built.series} colors={built.colors} legend={legend} locale="en" hidden={new Set()} onToggle={() => undefined} />);
    expect(container.querySelector(".max-h-48")).not.toBeNull();
    unmount();
    const before = window.matchMedia;
    window.matchMedia = ((query: string) => ({ matches: query.includes("max-width"), addEventListener() {}, removeEventListener() {} })) as unknown as typeof window.matchMedia;
    try {
      const phone = render(<Legend series={built.series} colors={built.colors} legend={legend} locale="en" hidden={new Set()} onToggle={() => undefined} />);
      expect(phone.container.querySelector(".max-h-48, .overflow-y-auto")).toBeNull();
      expect(screen.getAllByRole("row")).toHaveLength(built.series.length + 1);
    } finally {
      window.matchMedia = before;
    }
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

  it("orders rows by Grafana's saved sortBy, by display name, an empty cell last", () => {
    const air = [{ fields: [
      { name: "name", type: "string" as const, values: ["Mäkelänkatu", "Kallio 2", "Länsisatama 4", "Vartiokylä"] },
      { name: "pm25", type: "number" as const, values: [27.4, 12.1, 48.8, null] },
    ] }];
    const renamed = { overrides: [{ matcher: { id: "byName", options: "pm25" }, properties: [{ id: "displayName", value: "PM2.5" }] }] };
    const names = (sortBy?: { displayName?: string; desc?: boolean }[]) => tableOf(air, renamed, sortBy).entities.map((row) => (row.name as { value: string }).value);
    expect(names([{ displayName: "PM2.5", desc: true }])).toEqual(["Länsisatama 4", "Mäkelänkatu", "Kallio 2", "Vartiokylä"]);
    expect(names([{ displayName: "PM2.5" }])).toEqual(["Kallio 2", "Mäkelänkatu", "Länsisatama 4", "Vartiokylä"]);
    expect(names([{ displayName: "no such column", desc: true }])).toEqual(["Mäkelänkatu", "Kallio 2", "Länsisatama 4", "Vartiokylä"]);
    expect(names()).toEqual(["Mäkelänkatu", "Kallio 2", "Länsisatama 4", "Vartiokylä"]);
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

  it("colours a color-text cell no mapping names in its field's base colour, Grafana's green by default", () => {
    const settings = settingsFor({ defaults: { custom: { cellOptions: { type: "color-text" } } } }, { name: "Status", type: "string", values: [] });
    render(<TableCellValue settings={settings} value="withIncidence" locale="en" />);
    expect(screen.getByText("withIncidence")).toHaveStyle({ color: colorOf("green") });
  });

  it("keeps a time at the left: Grafana aligns a time field left though it is a number", () => {
    const { container } = render(<TableCellValue settings={{ ...settingsFor(undefined, { name: "Time", type: "time", values: [] }), unit: "dateTimeAsSystem" }} value={1794117600000} locale="en" />);
    expect(container.firstElementChild).not.toHaveClass("text-right");
  });

  it("draws every column on a phone as plain headers and cells, numbers at the right", () => {
    const { columns, entities } = tableOf(frames.stations, fieldConfig);
    render(<PhoneTable title="Stations" columns={columns} entities={entities} locale="en" />);
    const table = screen.getByRole("table", { name: "Stations" });
    expect(within(table).getAllByRole("columnheader").map((head) => head.textContent)).toEqual(columns.map((column) => column.label));
    expect(within(table).getAllByRole("row")).toHaveLength(entities.length + 1);
    expect(within(table).queryByRole("button")).toBeNull();
    // Heads keep the field's own case, so a unit such as t/km² keeps its meaning.
    expect(table).toHaveClass("[&_th]:normal-case");
  });

  it("puts a number at the cell's right edge and text at the left, unless custom.align says", () => {
    const { container, rerender } = render(<TableCellValue settings={settingsFor(undefined, { name: "n", type: "number", values: [] })} value={12} locale="en" />);
    expect(container.firstElementChild).toHaveClass("text-right");
    rerender(<TableCellValue settings={settingsFor(undefined, { name: "s", type: "string", values: [] })} value="Kallio" locale="en" />);
    expect(container.firstElementChild).not.toHaveClass("text-right");
    rerender(<TableCellValue settings={settingsFor({ defaults: { custom: { align: "center" } } }, { name: "n", type: "number", values: [] })} value={12} locale="en" />);
    expect(container.firstElementChild).toHaveClass("text-center");
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

describe("Grafana's auto points", () => {
  it("draws points while they are no closer than uPlot's 10 px at line width 1", () => {
    // A 540 px plot holds 55 points at 10 px; Grafana's 1440 shots dot their 50-point lines.
    expect(autoPoints(55, 540, 1)).toBe(true);
    expect(autoPoints(56, 540, 1)).toBe(false);
    expect(autoPoints(25, 540, 3)).toBe(true);
    expect(autoPoints(31, undefined, 1)).toBe(false);
  });

  it("connects over nulls as spanNulls says: all when true, gaps up to its ms when a number", () => {
    const points = [0, 1, 2, 3, 6, 7].map((t) => ({ t: t * 1000, v: t === 2 || t === 6 ? null : t }));
    expect(spanned(points, false)).toEqual(points);
    expect(spanned(points, true).map((point) => point.t / 1000)).toEqual([0, 1, 3, 7]);
    // 1 s..3 s is 2 s, within 2500 ms; 3 s..7 s is 4 s, past it, so that null still breaks.
    expect(spanned(points, 2500).map((point) => point.t / 1000)).toEqual([0, 1, 3, 6, 7]);
  });

  it("dots a lone reading between gaps where the line draws no points", () => {
    const t = (i: number) => frames.FROM + i * 60_000;
    const gappy: Frame[] = [{ name: "a", fields: [{ name: "Time", type: "time", values: Array.from({ length: 40 }, (_, i) => t(i)) }, { name: "a", type: "number", values: Array.from({ length: 40 }, (_, i) => (i === 20 ? 5 : i > 18 && i < 22 ? null : i)) }] }];
    const built = timeSeriesSpec(gappy, { options: {}, locale: "en", theme, height: 200, width: 400, from: frames.FROM, to: frames.FROM + 40 * 60_000 });
    const layers = (built.spec as { layer: { mark: { type: string }; transform?: unknown[] }[] }).layer;
    expect(layers.filter((layer) => layer.mark.type === "point" && JSON.stringify(layer.transform).includes("lone"))).toHaveLength(1);
    expect((built.spec as { data: { values: { lone?: boolean; v: number | null }[] } }).data.values.filter((value) => value.lone).map((value) => value.v)).toEqual([5]);
  });
});

describe("Grafana's value scale", () => {
  // Grafana 12.4.14's assistant board: lastNotNull 131, 134 and 163 over series ranging 29 to 212
  // fill 56, 57 and 73 % of their bars, all red past the default threshold of 80.
  const statuses: Frame[] = ["2xx", "4xx", "5xx"].map((name, at) => ({
    name,
    fields: [
      { name: "Time", type: "time", values: [1, 2, 3] },
      { name: "Value", type: "number", values: [[29, 33, 30][at], [208, 212, 209][at], [131, 134, 163][at]] },
    ],
  }));

  it("scales to every value of every series and colours by the default thresholds", () => {
    const shown = shownValues(statuses, { defaults: { unit: "short" }, overrides: [] }, {}, "en");
    expect(shown.map((one) => [one.min, one.max, Math.round(one.share * 100)])).toEqual([
      [29, 212, 56],
      [29, 212, 57],
      [29, 212, 73],
    ]);
    expect(new Set(shown.map((one) => one.color))).toEqual(new Set([colorOf("red")]));
    expect(globalRange([])).toBeUndefined();
  });

  it("holds green to halfway to the threshold, then blends into red, as Grafana's gradient bars do", () => {
    // 80 sits at 28 % of 29..212; the blend starts halfway there, 14 % of the scale, 25 % of a 56 % bar.
    expect(barGradient(DEFAULT_THRESHOLDS, 131, 29, 212, (131 - 29) / 183, true)).toBe(`linear-gradient(to top, ${colorOf("green")}, ${colorOf("green")} 25.0%, ${colorOf("red")})`);
    // A value under the threshold stays in the base colour.
    expect(barGradient(DEFAULT_THRESHOLDS, 50, 0, 100, 0.5, false)).toBe(`linear-gradient(to right, ${colorOf("green")}, ${colorOf("green")})`);
  });

  it("stands the bars up in a panel wider than tall, the value in its colour above each", () => {
    render(<BarGaugePanel title="Runs" frames={statuses} fieldConfig={{ defaults: {}, overrides: [] }} options={{ orientation: "vertical" }} height={200} locale="en" />);
    const meter = screen.getByRole("meter", { name: "5xx" });
    expect(meter).toHaveAttribute("aria-valuemin", "29");
    expect(meter.firstElementChild).toHaveStyle({ height: "73%" });
    expect(screen.getByText("163")).toHaveStyle({ color: colorOf("red") });
    // Grafana's default thresholds need no key.
    expect(screen.queryByRole("list", { name: "Colours by value" })).toBeNull();
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

  it("climbs Grafana's time-unit ladders, plural only on a scaled word, as its value formats do", () => {
    // slo panel 6: Grafana 12.4.14 prints 49.704 and 53.163 days as 7.10 and 7.59 weeks.
    expect(formatUnit(49.704, "d")).toBe("7.10 weeks");
    expect(formatUnit(53.163, "d")).toBe("7.59 weeks");
    expect(formatUnit(3, "d")).toBe("3 day");
    expect(formatUnit(400, "d")).toBe("1.10 years");
    expect(formatUnit(0, "s")).toBe("0 s");
    expect(formatUnit(0.25, "s")).toBe("250 ms");
    expect(formatUnit(90, "s")).toBe("1.50 mins");
    expect(formatUnit(7200, "s")).toBe("2 hours");
    expect(formatUnit(1500, "ms")).toBe("1.50 s");
    expect(formatUnit(45, "m")).toBe("45 min");
    expect(formatUnit(30, "h")).toBe("1.25 days");
    expect(formatUnit(2500, "ns")).toBe("2.50 µs");
    // `locale` is toLocaleString (helsinki cyclists: 1,514), `none` Grafana's toFixed (1514).
    expect(formatUnit(1514, "locale", 0)).toBe("1,514");
    expect(formatUnit(1514, "none")).toBe("1514");
  });

  it("keeps an override's unit for its field only", () => {
    const config = { defaults: { unit: "percent" }, overrides: [{ matcher: { id: "byName", options: "pm25" }, properties: [{ id: "unit", value: "bytes" }] }] };
    expect(settingsFor(config, { name: "pm25", type: "number", values: [] }).unit).toBe("bytes");
    expect(settingsFor(config, { name: "no2", type: "number", values: [] }).unit).toBe("percent");
  });
});

describe("the builder's kinds, drawn from the file the Portal wrote and frames recorded on dev", () => {
  // The panels as `grafana_builder_tests` pins them, and helsinki's recorded NGSI-LD answers:
  // the 6-hour pm25 averages per station, and the bike stations' current values.
  const file = builderFile as { panels: PanelDefinition[] };
  const panel = (type: string): PanelDefinition => {
    const found = file.panels.find((one) => one.type === type);
    if (!found) throw new Error(`no ${type} in the builder fixture`);
    return found;
  };
  const history = (recordedPm25 as { frames: Frame[] }).frames;
  // A bar chart reads the bars' names and one value: the recorded read narrowed to them.
  const bikes: Frame[] = (recordedBikes as { frames: Frame[] }).frames.map((frame) => ({
    ...frame,
    fields: frame.fields.filter((field) => ["name", "availableBikeNumber"].includes(field.name)),
  }));
  const view = (one: PanelDefinition, data: Frame[]) =>
    render(<PanelView panel={one} data={{ frames: data }} from={frames.FROM} to={frames.TO} locale="en" height={240} />);

  it("draws a bar gauge per station on the saved 0 to 40 scale, LCD and horizontal", () => {
    const bars = panel("bargauge");
    expect(bars.options).toMatchObject({ displayMode: "lcd", orientation: "horizontal" });
    view(bars, history);
    const meters = screen.getAllByRole("meter");
    expect(meters).toHaveLength(history.length);
    for (const meter of meters) {
      expect(meter).toHaveAttribute("aria-valuemin", "0");
      expect(meter).toHaveAttribute("aria-valuemax", "40");
    }
  });

  it("scales a gauge to its saved minimum and maximum, not to the data", () => {
    const gauge = panel("gauge");
    expect(gauge.fieldConfig?.defaults).toMatchObject({ min: 0, max: 40 });
    const shown = shownValues(history, gauge.fieldConfig ?? { defaults: {}, overrides: [] }, gauge.options ?? {}, "en");
    expect(shown.every((one) => one.min === 0 && one.max === 40)).toBe(true);
  });

  it("draws a donut with a slice per station from the reduced history", async () => {
    const pie = panel("piechart");
    const built = pieChartSpec(history, { fieldConfig: pie.fieldConfig, options: pie.options, locale: "en", theme, height: 200 });
    expect(built.slices).toHaveLength(history.length);
    expect((built.spec.mark as { innerRadius: number }).innerRadius).toBeGreaterThan(0);
    const svg = await svgOf(built.spec);
    expect(count(svg, /class="mark-arc/)).toBeGreaterThanOrEqual(1);
  });

  it("draws a bar per station named by its name attribute", async () => {
    const chart = panel("barchart");
    expect(chart.options?.xField).toBe("name");
    const built = barChartSpec(bikes, { fieldConfig: chart.fieldConfig, options: chart.options, locale: "en", theme, height: 200 });
    expect(built.names).toEqual(["availableBikeNumber"]);
    const svg = await svgOf(built.spec);
    expect(count(svg, /<path [^>]*role="graphics-symbol"/)).toBe(bikes[0].fields[0].values.length);
    expect(svg).toContain("Kaivopuisto");
  });

  it("shows a text panel as plain text with its Markdown taken out", () => {
    view(panel("text"), []);
    const card = screen.getByRole("region", { name: "About these stations" });
    expect(card).toHaveTextContent("Bike stations Free bikes per station, read every 5 minutes.");
    expect(card.textContent).not.toMatch(/[#*]/);
    expect(plainText("## Hi\n- [a link](https://x.example) and <b>bold</b>\n> `code`")).toBe("Hi\n• a link and bold\ncode");
  });
});
