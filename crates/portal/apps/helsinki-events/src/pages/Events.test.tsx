import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { EVENTS } from "../fixtures/events";

// T-2923, AP-138: the events page draws in colour: a chart per question, the events on the map
// in their register's colour, and a click on a bar picks the list out.
type Handler = (event: Record<string, unknown>) => void;
const maps: Array<{
  setData: ReturnType<typeof vi.fn>;
  addSource: ReturnType<typeof vi.fn>;
  easeTo: ReturnType<typeof vi.fn>;
  load?: () => void;
  clicks: Record<string, Handler>;
}> = [];
vi.mock("maplibre-gl", () => {
  class Map {
    setData = vi.fn();
    easeTo = vi.fn();
    clicks: Record<string, Handler> = {};
    fitBounds = vi.fn();
    addControl = vi.fn();
    removeControl = vi.fn();
    setPaintProperty = vi.fn();
    addSource = vi.fn();
    addLayer = vi.fn();
    getSource = vi.fn(() => ({ setData: this.setData, getClusterExpansionZoom: vi.fn(async () => 13) }));
    remove = vi.fn();
    load?: () => void;
    constructor() {
      maps.push(this);
    }
    on(event: string, layer: unknown, handler?: unknown) {
      if (event === "load" && typeof layer === "function") this.load = layer as () => void;
      if (event === "click" && typeof layer === "string") this.clicks[layer] = handler as Handler;
    }
  }
  return { Map, setWorkerUrl: vi.fn() };
});
vi.mock("@deck.gl/mapbox", () => ({ MapboxOverlay: class {} }));
vi.mock("@deck.gl/layers", () => ({ ScatterplotLayer: class {} }));
vi.mock("@deck.gl/aggregation-layers", () => ({ HexagonLayer: class {}, GridLayer: class {} }));

type Click = (params: { name?: string }) => void;
const charts: Array<{ element: HTMLElement; setOption: ReturnType<typeof vi.fn>; click?: Click }> = [];
vi.mock("echarts", () => ({
  init: vi.fn((element: HTMLElement) => {
    const chart: (typeof charts)[number] = { element, setOption: vi.fn() };
    const api = {
      setOption: chart.setOption,
      resize: vi.fn(),
      dispose: vi.fn(),
      on: vi.fn((event: string, handler: Click) => {
        if (event === "click") chart.click = handler;
      }),
    };
    charts.push(chart);
    return api;
  }),
}));

import { Events } from "./Events";

const READ = {
  permissions: [{ resource: { type: "Event" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const }],
  prohibitions: [],
};

function events(entities: Row[] = EVENTS, refuse?: () => { status: number; body: unknown } | null) {
  const client = stubClient({ entities, access: READ, refuse }, { appName: "helsinki-events" });
  render(
    <JcProvider client={client}>
      <Events />
    </JcProvider>,
  );
  return client;
}

const chartOf = (title: string) => {
  const figure = screen.getByText(title).closest("figure") as HTMLElement;
  const chart = charts.filter((c) => figure.contains(c.element)).at(-1);
  expect(chart, title).toBeDefined();
  return chart!;
};
const optionOf = (title: string): Record<string, any> => chartOf(title).setOption.mock.calls.at(-1)![0];
/** Clicks a bar of the chart captioned `title`, as ECharts reports it, and lets React draw the result. */
const clickBar = async (title: string, name: string) => {
  // The chart is built after the list shows, a beat later on a loaded runner.
  const click = await waitFor(() => {
    const { click } = chartOf(title);
    expect(click, `${title} listens for clicks`).toBeDefined();
    return click!;
  });
  act(() => click({ name }));
};
const listed = () => within(screen.getByRole("list", { name: "Upcoming events" })).getAllByRole("heading").map((h) => h.textContent);

describe("the events page", () => {
  beforeEach(() => {
    maps.length = 0;
    charts.length = 0;
    // The page opens on today in Helsinki; the fixtures are in October 2030.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date("2030-10-20T06:00:00Z"));
  });
  afterEach(() => vi.useRealTimers());

  it("asks the endpoint only for the events that have not ended, and lists the rest soonest first", async () => {
    const client = events();
    await waitFor(() => expect(listed()).toHaveLength(5));
    expect(listed()).toEqual(["Workshop for Families", "Organ concert", "Story hour", "Dance workshop", "Jazz at Stoa"]);
    expect(client.transport.calls.some((call) => decodeURIComponent(call.path).includes("q=endDate>="))).toBe(true);
    expect(screen.getByText("5 of 6 events")).toBeInTheDocument();
  });

  it("charts the events per day for 30 days and per register, each register in its own colour", async () => {
    events();
    await waitFor(() => expect(optionOf("Events per day, next 30 days").series[0].data).toHaveLength(30));
    const perDay = optionOf("Events per day, next 30 days");
    expect(perDay.xAxis.data[0]).toBe("2030-10-20");
    expect(perDay.series[0].data.slice(0, 4)).toEqual([2, 0, 1, 1]);
    // Each chart is built on its own beat (T-2994): wait for this one too.
    await waitFor(() => expect(optionOf("Events by register").yAxis.data).toHaveLength(3));
    const registers = optionOf("Events by register");
    expect(registers.yAxis.data).toEqual(["City of Helsinki", "Culture centres", "City of Espoo"]);
    const colours = registers.series[0].data.map((bar: { itemStyle: { color: string } }) => bar.itemStyle.color);
    expect(new Set(colours).size).toBe(3);
    const legend = screen.getByLabelText("Map colour: the register that publishes the event");
    expect(within(legend).getAllByRole("listitem").map((item) => item.textContent)).toEqual(registers.yAxis.data);
  });

  // T-3029: a register of events that opened long ago still has a bar on every day they run.
  it("charts a long-running event on each day and lists it for a clicked day", async () => {
    const running = {
      ...EVENTS[0],
      id: "urn:ngsi-ld:Event:hel.fi:helsinki:helsinki-town",
      name: "Children's Town",
      startDate: "2001-01-01T08:00:00Z",
      endDate: "2050-12-31T20:00:00Z",
    } as Row;
    events([running]);
    await waitFor(() => expect(optionOf("Events per day, next 30 days").series[0].data).toHaveLength(30));
    expect(optionOf("Events per day, next 30 days").series[0].data.every((count: number) => count === 1)).toBe(true);
    await clickBar("Events per day, next 30 days", "2030-10-25");
    expect(listed()).toEqual(["Children's Town"]);
    // Outside the shell there is no panel to open the event in (App.test.tsx opens it in one).
    fireEvent.click(screen.getByRole("button", { name: "Children's Town" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says the 30 days hold no event instead of drawing an empty axis", async () => {
    // Upcoming by its end, but it only starts after the window.
    const later = { ...EVENTS[0], startDate: "2030-12-01T10:00:00Z", endDate: "2030-12-01T12:00:00Z" } as Row;
    events([later]);
    await waitFor(() => expect(listed()).toHaveLength(1));
    const figure = screen.getByText("Events per day, next 30 days").closest("figure") as HTMLElement;
    expect(within(figure).getByText("No event takes place on these 30 days.")).toBeInTheDocument();
    expect(within(figure).queryByRole("img")).toBeNull();
  });

  it("puts one feature per located event on the map, coloured by its register", async () => {
    events();
    await waitFor(() => expect(maps.length).toBeGreaterThan(0));
    const map = maps[0];
    map.load?.();
    // A draw before the events are read carries none; wait for the one that carries them (T-2994).
    await waitFor(() => expect(map.setData.mock.calls.at(-1)?.[0].features).toHaveLength(4));
    const features = map.setData.mock.calls.at(-1)![0].features;
    // Five are upcoming, and the cancelled jazz evening has no location.
    expect(features).toHaveLength(4);
    const colours = new Set(features.map((f: { properties: { color: string } }) => f.properties.color));
    expect(colours.size).toBe(3);
  });

  it("clusters close markers and zooms in on a clicked cluster", async () => {
    events();
    await waitFor(() => expect(maps.length).toBeGreaterThan(0));
    const map = maps[0];
    map.load?.();
    expect(map.addSource).toHaveBeenCalledWith("jc-rows", expect.objectContaining({ cluster: true }));
    map.clicks["jc-clusters"]({ features: [{ properties: { cluster_id: 7 }, geometry: { type: "Point", coordinates: [24.94, 60.17] } }] });
    await waitFor(() => expect(map.easeTo).toHaveBeenCalledWith({ center: [24.94, 60.17], zoom: 13 }));
  });

  it("shows only the day or the register whose bar is clicked, and the chip puts them back", async () => {
    events();
    await waitFor(() => expect(listed()).toHaveLength(5));
    await clickBar("Events per day, next 30 days", "2030-10-22");
    expect(listed()).toEqual(["Story hour"]);
    fireEvent.click(screen.getByRole("button", { name: "Show every day, not only 22 Oct" }));
    await waitFor(() => expect(listed()).toHaveLength(5));
    await clickBar("Events by register", "Culture centres");
    expect(listed()).toEqual(["Dance workshop", "Jazz at Stoa"]);
    fireEvent.click(screen.getByRole("button", { name: "Show every register, not only Culture centres" }));
    await waitFor(() => expect(listed()).toHaveLength(5));
    // A second click on the same bar is the way back too.
    await clickBar("Events by register", "Culture centres");
    await clickBar("Events by register", "Culture centres");
    expect(listed()).toHaveLength(5);
  });

  it("searches, narrows by date and says when nothing matches", async () => {
    events();
    await waitFor(() => expect(listed()).toHaveLength(5));
    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: "turunlinnantie" } });
    expect(listed()).toEqual(["Dance workshop", "Jazz at Stoa"]);
    fireEvent.change(screen.getByLabelText("To"), { target: { value: "2030-10-31" } });
    expect(listed()).toEqual(["Dance workshop"]);
    // From after the dance workshop leaves nothing in the range.
    fireEvent.change(screen.getByLabelText("From"), { target: { value: "2030-10-30" } });
    expect(screen.getByText("No event matches the filters.")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("From"), { target: { value: "2030-10-20" } });
    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), { target: { value: "opera" } });
    expect(screen.getByText("No event matches the filters.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    expect(listed()).toHaveLength(5);
  });

  it("marks a cancelled event and links only an https source", async () => {
    events();
    await waitFor(() => expect(listed()).toHaveLength(5));
    const card = (name: string) => screen.getByRole("heading", { name }).closest("li") as HTMLElement;
    expect(within(card("Jazz at Stoa")).getByText("Cancelled")).toBeInTheDocument();
    expect(within(card("Organ concert")).queryByText("Cancelled")).toBeNull();
    const source = within(card("Organ concert")).getByRole("link", { name: "Source" });
    expect(source).toHaveAttribute("href", "https://api.hel.fi/linkedevents/v1/");
    expect(source).toHaveAttribute("rel", "noopener noreferrer");
    fireEvent.click(source);
    expect(within(card("Story hour")).queryByRole("link")).toBeNull();
  });

  it("says there are no upcoming events when the endpoint has none", async () => {
    events([]);
    expect(await screen.findByText("No upcoming events.")).toBeInTheDocument();
    const registers = screen.getByText("Events by register").closest("figure") as HTMLElement;
    expect(within(registers).getByText("No event matches.")).toBeInTheDocument();
    // No axis of 30 empty days: the card says so (T-3029).
    const days = screen.getByText("Events per day, next 30 days").closest("figure") as HTMLElement;
    expect(within(days).getByText("No event takes place on these 30 days.")).toBeInTheDocument();
  });

  it("says the events could not be read, with the status, and reads them again on Retry", async () => {
    let fail = true;
    events(EVENTS, () => (fail ? { status: 503, body: { title: "Unavailable" } } : null));
    const alert = await screen.findByRole("alert");
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    expect(screen.getAllByText("Nothing to chart until the events are read.")).toHaveLength(2);
    expect(alert).toHaveTextContent("The events could not be read (HTTP 503). Try again later.");
    fail = false;
    fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(listed()).toHaveLength(5));
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
