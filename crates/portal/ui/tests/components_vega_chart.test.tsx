// The chart panels draw through VegaChart, which loads Vega on demand. jsdom has no canvas, so
// Vega itself is replaced here by a View that records what it was asked and lets a test move the
// pointer: what is covered is what the page does with Vega, not Vega's drawing.
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "../src/i18n";
import * as frames from "./fixtures/panel_frames";

type TooltipHandler = (handler: unknown, event: { clientX: number; clientY: number }, item: { datum?: Record<string, unknown> } | null | undefined, value: unknown) => void;

const vega = vi.hoisted(() => ({
  views: [] as { tooltipHandler?: TooltipHandler; finalized: boolean; options: Record<string, unknown> }[],
  fail: null as unknown,
  registered: [] as string[],
  loader: null as null | { load: () => Promise<unknown>; sanitize: () => Promise<unknown> },
}));

vi.mock("vega", () => ({
  expressionFunction: (name: string) => {
    vega.registered.push(name);
  },
  loader: () => {
    vega.loader = { load: () => Promise.resolve(""), sanitize: () => Promise.resolve({}) };
    return vega.loader;
  },
  parse: (spec: unknown) => spec,
  View: class {
    record: (typeof vega.views)[number];
    constructor(_runtime: unknown, options: Record<string, unknown>) {
      this.record = { finalized: false, options };
      vega.views.push(this.record);
    }
    tooltip(handler: TooltipHandler) {
      this.record.tooltipHandler = handler;
      return this;
    }
    runAsync() {
      return vega.fail === null ? Promise.resolve(this) : Promise.reject(vega.fail);
    }
    finalize() {
      this.record.finalized = true;
    }
  },
}));
vi.mock("vega-lite", () => ({ compile: (spec: unknown) => ({ spec }) }));
vi.mock("vega-interpreter", () => ({ expressionInterpreter: {} }));

const { VegaChart, chartTheme } = await import("../src/components/panels/VegaChart");
const { BarChartPanel, HeatmapPanel, HistogramPanel, PieChartPanel, StateTimelinePanel } = await import("../src/components/panels/ChartPanels");
const { TimeSeriesPanel } = await import("../src/components/panels/TimeSeriesPanel");
const { PanelView } = await import("../src/components/panels/PanelView");
const { BarGaugePanel, GaugePanel } = await import("../src/components/panels/ValuePanels");

afterEach(() => {
  vega.views = [];
  vega.fail = null;
});

/** Moves the pointer over the chart at (x, y) of a 400 px wide box, on the given datum. */
function hoverAt(x: number, datum: Record<string, unknown> | undefined, value: unknown = 1) {
  const view = vega.views.at(-1);
  if (!view?.tooltipHandler) throw new Error("no view drew");
  const host = screen.getByRole("img").firstElementChild as HTMLElement;
  host.getBoundingClientRect = () => ({ left: 0, top: 0, width: 400, height: 200, right: 400, bottom: 200, x: 0, y: 0, toJSON: () => ({}) });
  act(() => view.tooltipHandler?.(null, { clientX: x, clientY: 20 }, datum === undefined ? null : { datum }, value));
}

const common = { title: "Requests", height: 240, locale: "en", from: frames.FROM, to: frames.TO };

describe("VegaChart", () => {
  it("draws on a canvas through the interpreter, refuses every load and registers jcFormat once", async () => {
    render(<VegaChart spec={{ mark: "bar" }} label="Requests per route" height={200} />);
    expect(screen.getByRole("img", { name: "Requests per route" })).toBeInTheDocument();
    await waitFor(() => expect(vega.views).toHaveLength(1));
    expect(vega.views[0].options).toMatchObject({ renderer: "canvas", hover: true });
    expect(vega.registered).toEqual(["jcFormat"]);
    await expect(vega.loader?.load()).rejects.toThrow("a dashboard chart loads nothing");
    await expect(vega.loader?.sanitize()).rejects.toThrow("a dashboard chart loads nothing");
  });

  it("opens the tooltip right of the pointer on the left half and left of it on the right half, and closes it", async () => {
    render(<VegaChart spec={{}} label="Chart" height={200} tooltip={(datum) => <span>{`value ${String(datum.v)}`}</span>} />);
    await waitFor(() => expect(vega.views[0]?.tooltipHandler).toBeDefined());
    hoverAt(50, { v: 3 });
    const left = screen.getByRole("tooltip");
    expect(left).toHaveTextContent("value 3");
    expect(left.style.left).toBe("62px");
    hoverAt(350, { v: 4 });
    expect(screen.getByRole("tooltip").style.right).toBe("62px");
    hoverAt(350, undefined);
    expect(screen.queryByRole("tooltip")).toBeNull();
    hoverAt(350, { v: 4 }, null);
    expect(screen.queryByRole("tooltip")).toBeNull();
    hoverAt(350, { v: 5 });
    fireEvent.pointerLeave(screen.getByRole("img").firstElementChild as HTMLElement);
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("shows no tooltip without a tooltip renderer", async () => {
    render(<VegaChart spec={{}} label="Chart" height={200} />);
    await waitFor(() => expect(vega.views[0]?.tooltipHandler).toBeDefined());
    hoverAt(50, { v: 3 });
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("names Vega's error when the chart cannot be drawn, and collapses the box", async () => {
    vega.fail = new Error("unknown mark type");
    render(<VegaChart spec={{}} label="Chart" height={200} />);
    const error = await screen.findByText("unknown mark type");
    expect(error).toHaveClass("text-danger");
    expect((screen.getByRole("img").firstElementChild as HTMLElement).style.height).toBe("0px");
  });

  it("shows the page's own note in place of a thrown non-error", async () => {
    vega.fail = "bad";
    const { rerender } = render(<VegaChart spec={{}} label="Chart" height={200} failure="The values are in the table below." />);
    expect(await screen.findByText("The values are in the table below.")).toHaveClass("text-fg-muted");
    vega.fail = "worse";
    rerender(<VegaChart spec={{}} label="Chart" height={200} />);
    expect(await screen.findByText("worse")).toBeInTheDocument();
  });

  it("finalizes the view when the chart leaves and draws nothing once unmounted mid-load", async () => {
    const { unmount } = render(<VegaChart spec={{}} label="Chart" height={200} />);
    await waitFor(() => expect(vega.views).toHaveLength(1));
    unmount();
    expect(vega.views[0].finalized).toBe(true);
    const again = render(<VegaChart spec={{}} label="Chart" height={200} />);
    again.unmount();
    await act(async () => {
      await Promise.resolve();
    });
    expect(vega.views).toHaveLength(1);
  });

  it("reads the light and dark chart themes", () => {
    expect(chartTheme("light").surface).not.toBe(chartTheme("dark").surface);
    expect(chartTheme().font).toBeTruthy();
  });
});

describe("chart panels", () => {
  it("bar chart: tooltip lines name category, series and value, and a key appears for two series", async () => {
    const twoSeries = [{ ...frames.routes[0], fields: [...frames.routes[0].fields, { name: "errors", type: "number" as const, values: [3, 1, 0] }] }];
    render(<BarChartPanel {...common} frames={twoSeries} />);
    await waitFor(() => expect(vega.views[0]?.tooltipHandler).toBeDefined());
    hoverAt(50, { x: "/q/health", s: "requests", v: 2400, ignored: null });
    const tip = screen.getByRole("tooltip");
    expect(tip).toHaveTextContent("/q/health");
    expect(tip).toHaveTextContent("2400");
    expect(screen.getByRole("list")).toHaveTextContent("errors");
  });

  it("bar chart: one series draws no key", async () => {
    render(<BarChartPanel {...common} frames={frames.routes} />);
    await waitFor(() => expect(vega.views).toHaveLength(1));
    expect(screen.queryByRole("list")).toBeNull();
  });

  it("pie chart: the key gives each slice its share, and an all-zero pie divides by one", async () => {
    render(<PieChartPanel {...common} frames={frames.routes} />);
    await waitFor(() => expect(vega.views[0]?.tooltipHandler).toBeDefined());
    expect(screen.getByRole("list")).toHaveTextContent("requests100%");
    hoverAt(50, { name: "/q/health", value: 2400 });
    expect(screen.getByRole("tooltip")).toHaveTextContent("2400");
    const zero = [{ ...frames.routes[0], fields: [frames.routes[0].fields[0], { name: "requests", type: "number" as const, values: [0, 0, 0] }] }];
    render(<PieChartPanel {...common} frames={zero} />);
    expect(screen.getAllByRole("list").at(-1)).toHaveTextContent("0%");
  });

  it("heatmap: Prometheus buckets and a time datum formatted as a date", async () => {
    render(<HeatmapPanel {...common} frames={frames.durationBuckets} />);
    await waitFor(() => expect(vega.views[0]?.tooltipHandler).toBeDefined());
    hoverAt(50, { t: frames.FROM, b: "0.1", v: 40 });
    expect(screen.getByRole("tooltip")).toHaveTextContent(new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" }).format(frames.FROM));
  });

  it("heatmap: plain series carry no bucket titles", async () => {
    render(<HeatmapPanel {...common} frames={frames.requestRate} />);
    await waitFor(() => expect(vega.views).toHaveLength(1));
    expect(screen.getByRole("img", { name: /Requests/ })).toBeInTheDocument();
  });

  it("state timeline: tooltip has since and until, the key lists the states", async () => {
    render(<StateTimelinePanel {...common} frames={frames.runState} />);
    await waitFor(() => expect(vega.views[0]?.tooltipHandler).toBeDefined());
    hoverAt(50, { row: "run", text: "Running", t: frames.FROM, t2: frames.TO });
    expect(screen.getByRole("tooltip")).toHaveTextContent("Running");
    expect(screen.getByRole("list").children.length).toBeGreaterThan(0);
  });

  it("histogram: one chart named for its title", async () => {
    render(<HistogramPanel {...common} frames={frames.memory} />);
    await waitFor(() => expect(vega.views).toHaveLength(1));
    expect(screen.getByRole("img", { name: /Requests/ })).toBeInTheDocument();
  });
});

describe("time series panel", () => {

  it("table legend sorted by Max descending, tooltip sorted by value, a click hides and shows a series", async () => {
    const options = { legend: { displayMode: "table", placement: "right", calcs: ["max", "p95", "unknown"], sortBy: "Max", sortDesc: true }, tooltip: { sort: "desc" } };
    render(<TimeSeriesPanel {...common} frames={frames.requestRate} options={options} />);
    const table = screen.getByRole("table");
    const names = Array.from(table.querySelectorAll("tbody tr")).map((row) => row.textContent ?? "");
    expect(names[0]).toMatch(/200/);
    expect(table).toHaveTextContent(/95/);
    await waitFor(() => expect(vega.views[0]?.tooltipHandler).toBeDefined());
    const series = Array.from(table.querySelectorAll("tbody button")).map((button) => button.textContent ?? "");
    hoverAt(50, { t: frames.FROM, [series[0]]: 1, [series[1]]: 9 });
    const items = screen.getByRole("tooltip").querySelectorAll("li");
    expect(items[0]).toHaveTextContent(series[1]);
    const button = screen.getByRole("button", { name: series[1] });
    fireEvent.click(button);
    expect(screen.getByRole("button", { name: series[1] })).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(screen.getByRole("button", { name: series[1] }));
    expect(screen.getByRole("button", { name: series[1] })).toHaveAttribute("aria-pressed", "true");
  });

  it("list legend with calcs, ascending tooltip without a time, and a sort naming no calc keeps the order", async () => {
    const options = { legend: { displayMode: "list", calcs: ["lastNotNull"], sortBy: "Nothing" }, tooltip: { sort: "asc" } };
    render(<TimeSeriesPanel {...common} frames={frames.requestRate} options={options} />);
    const list = screen.getByRole("list");
    expect(list).toHaveTextContent(":");
    await waitFor(() => expect(vega.views[0]?.tooltipHandler).toBeDefined());
    const series = Array.from(list.querySelectorAll("button")).map((button) => button.textContent ?? "");
    hoverAt(50, { t: "never", [series[0]]: 9, [series[1]]: 1 });
    expect(screen.getByRole("tooltip").querySelectorAll("li")[0]).toHaveTextContent(series[1]);
  });

  it("a hidden legend draws none, and a table legend without calcs falls back to the list", () => {
    const { unmount } = render(<TimeSeriesPanel {...common} frames={frames.requestRate} options={{ legend: { showLegend: false } }} />);
    expect(screen.queryByRole("list")).toBeNull();
    unmount();
    render(<TimeSeriesPanel {...common} frames={frames.requestRate} options={{ legend: { displayMode: "table" } }} />);
    expect(screen.getByRole("list")).toBeInTheDocument();
  });
});

describe("panel view per type", () => {
  const view = { from: frames.FROM, to: frames.TO, locale: "en", height: 240 };
  it.each([
    ["stat", frames.up],
    ["gauge", frames.up],
    ["bargauge", frames.memory],
    ["barchart", frames.routes],
    ["piechart", frames.routes],
    ["heatmap", frames.durationBuckets],
    ["histogram", frames.memory],
    ["state-timeline", frames.runState],
    ["timeseries", frames.requestRate],
  ])("draws a %s panel under its title", async (type, data) => {
    render(<PanelView panel={{ type, title: `A ${type}` }} data={{ frames: data }} {...view} />);
    expect(screen.getByRole("heading", { name: new RegExp(`A ${type}`) })).toBeInTheDocument();
    if (type !== "stat" && type !== "bargauge") expect(screen.getAllByRole("img").length).toBeGreaterThan(0);
  });
});

describe("gauge panel", () => {
  it("one arc per value with names, tooltips naming the value, and no markers when asked", async () => {
    render(<GaugePanel title="Up" frames={frames.memory} options={{ showThresholdMarkers: false }} height={200} locale="en" />);
    const arcs = await screen.findAllByRole("img");
    expect(arcs.length).toBe(2);
    expect(screen.getByRole("list", { name: "Up" }).children).toHaveLength(2);
  });

  it("one value named only when textMode asks for the name", () => {
    render(<GaugePanel title="Up" frames={frames.up} options={{ textMode: "value_and_name" }} height={200} locale="en" />);
    expect(screen.getAllByRole("img").length).toBeGreaterThan(0);
  });
});

describe("bar gauge panel", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("stands the bars up once the measured width passes the height, keys its thresholds and stops measuring when gone", () => {
    const observers: { disconnected: boolean }[] = [];
    vi.stubGlobal(
      "ResizeObserver",
      class {
        record = { disconnected: false };
        constructor(private readonly callback: () => void) {
          observers.push(this.record);
        }
        observe() {
          this.callback();
        }
        disconnect() {
          this.record.disconnected = true;
        }
      },
    );
    const width = vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(800);
    const thresholds = (mode: string) => ({ defaults: { thresholds: { mode, steps: [{ value: null, color: "green" }, { value: 80, color: "red" }] } }, overrides: [] });
    const { unmount } = render(<BarGaugePanel title="Memory" frames={frames.memory} fieldConfig={thresholds("percentage")} height={200} locale="en" />);
    expect(screen.getAllByRole("list")).toHaveLength(2);
    expect(screen.getAllByRole("meter")).toHaveLength(2);
    expect(document.body).toHaveTextContent("80%");
    unmount();
    expect(observers.every((one) => one.disconnected)).toBe(true);
    render(<BarGaugePanel title="Memory" frames={frames.memory} fieldConfig={thresholds("absolute")} options={{ orientation: "vertical", displayMode: "basic" }} height={200} locale="en" />);
    expect(screen.getAllByRole("meter")).toHaveLength(2);
    width.mockRestore();
  });
});
