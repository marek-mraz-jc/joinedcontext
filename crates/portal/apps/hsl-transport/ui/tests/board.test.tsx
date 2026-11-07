/**
 * T-3895: vehicles of every mode with a filter, the departures of a chosen stop and HSL's service
 * alerts, on the map and per line, all read from this app's own backend.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let setData = vi.fn();

vi.mock("maplibre-gl", () => {
  class Map {
    on(event: string, handler: unknown) {
      if (event === "load") (handler as () => void)();
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    addImage = vi.fn();
    hasImage = () => false;
    resize = () => undefined;
    getBounds = () => ({ toArray: () => [[24.0, 59.9], [25.9, 60.6]] });
    getSource = () => ({ setData });
    remove = vi.fn();
  }
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

import { Alerts, App, Departures } from "../src/App";
import { alertPlaces, alertsFor, getAlerts, getDepartures, minutesUntil, ofModes, when, type AlertView, type StopBoard, type Vehicle } from "../src/api";

const NOW = Date.parse("2026-10-12T08:00:00Z");

const fleet: Vehicle[] = [
  { id: "urn:ngsi-ld:Vehicle:hel.fi:helsinki:22-1", coordinates: [24.9, 60.2], refLine: "2550", mode: "bus" },
  { id: "urn:ngsi-ld:Vehicle:hel.fi:helsinki:40-3", coordinates: [24.95, 60.17], refLine: "1009", mode: "tram" },
  { id: "urn:ngsi-ld:Vehicle:hel.fi:helsinki:40-4", coordinates: [24.94, 60.16], refLine: "1004", mode: "tram" },
  { id: "urn:ngsi-ld:Vehicle:hel.fi:helsinki:50-1", coordinates: [25.08, 60.21], refLine: "31M1", mode: "metro" },
  { id: "urn:ngsi-ld:Vehicle:hel.fi:helsinki:12-9", coordinates: [24.99, 60.29], refLine: "4570" },
];

const stops: StopBoard[] = [
  { id: "urn:ngsi-ld:GtfsStop:hel.fi:helsinki:2222", name: "Tapiola", departures: [
    { line: "2550", headsign: "Itäkeskus", expected: "2026-10-12T08:04:30Z", status: "scheduled", delay: 60 },
    { line: "31M1", expected: "2026-10-12T08:06:00Z", status: "canceled" },
  ] },
  { id: "urn:ngsi-ld:GtfsStop:hel.fi:helsinki:1020601", name: "Rautatientori", departures: [
    { line: "31M2", headsign: "Vuosaari", expected: "2026-10-12T07:59:00Z", status: "scheduled" },
    { line: "1009", expected: "2026-10-12T08:02:00Z", status: "skipped" },
  ] },
];

const alerts: AlertView[] = [
  { id: "urn:ngsi-ld:Alert:hel.fi:helsinki:hsl-1", title: "Stop closed", severity: "high", lines: [], stops: ["1392501"],
    places: [{ stop: "1392501", coordinates: [25.03, 60.27] }] },
  { id: "urn:ngsi-ld:Alert:hel.fi:helsinki:hsl-2", title: "<b>Detour</b>", body: "<img src=x onerror=alert(1)>", severity: "medium",
    lines: ["100HE", "2550"], stops: [], places: [] },
];

class StubEventSource {
  addEventListener() {}
  close() {}
}

function backend(answers: Record<string, unknown>) {
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    const path = Object.keys(answers).find((tail) => url.endsWith(tail));
    return path === undefined ? new Response("", { status: 404 }) : new Response(JSON.stringify(answers[path]), { status: 200 });
  }));
}

beforeEach(() => {
  setData = vi.fn();
  vi.stubGlobal("EventSource", StubEventSource);
  backend({ "api/vehicles": fleet, "api/departures": stops, "api/alerts": alerts });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("the modes", () => {
  it("count a vehicle without a mode as a bus and keep only the chosen ones", () => {
    expect(ofModes(fleet, new Set(["bus"])).map((v) => v.id.slice(-4))).toEqual(["22-1", "12-9"]);
    expect(ofModes(fleet, new Set(["tram", "metro"]))).toHaveLength(3);
    expect(ofModes(fleet, new Set())).toEqual([]);
  });

  it("filter the map, the status and the lists when a mode is ticked off", async () => {
    render(<App />);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("5 vehicles"));
    const show = screen.getByRole("group", { name: "Show" });
    expect(within(show).getByRole("checkbox", { name: "tram (2)" })).toBeChecked();
    await userEvent.click(within(show).getByRole("checkbox", { name: "tram (2)" }));
    expect(screen.getByRole("status")).toHaveTextContent("3 vehicles");
    const drawn = setData.mock.calls.map(([data]) => data as { features: Array<{ properties: { refLine?: string } }> }).at(-1);
    expect(drawn?.features.map((feature) => feature.properties.refLine)).not.toContain("1009");
    for (const mode of ["bus", "metro", "train", "ferry"]) {
      await userEvent.click(within(show).getByRole("checkbox", { name: new RegExp(`^${mode} `) }));
    }
    expect(screen.getByRole("status")).toHaveTextContent("No vehicles of the chosen modes");
    await userEvent.click(within(show).getByRole("checkbox", { name: "tram (2)" }));
    expect(screen.getByRole("status")).toHaveTextContent("2 vehicles");
  });
});

describe("the departures", () => {
  it("read in minutes, now for a time passed, the clock an hour ahead, nothing for a bad time", () => {
    expect(minutesUntil("2026-10-12T08:04:30Z", NOW)).toBe(4);
    expect(when("2026-10-12T08:00:59Z", NOW)).toBe("now");
    expect(when("2026-10-12T07:58:00Z", NOW)).toBe("now");
    expect(when("2026-10-12T08:01:00Z", NOW)).toBe("1 min");
    expect(when("2026-10-12T09:15:00Z", NOW)).toBe("12:15");
    expect(when("tomorrow", NOW)).toBe("");
    expect(minutesUntil("tomorrow", NOW)).toBeUndefined();
  });

  it("show the first stop, switch to a chosen one, and say a cancelled or skipped call", async () => {
    render(<Departures stops={stops} now={NOW} />);
    let list = screen.getByRole("list", { name: "Departures from Tapiola" });
    expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["2550Itäkeskus4 min", "31M1cancelled"]);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Stop" }), "Rautatientori");
    list = screen.getByRole("list", { name: "Departures from Rautatientori" });
    expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["31M2Vuosaarinow", "1009skips this stop"]);
  });

  it("say so before the first read", () => {
    render(<Departures stops={[]} />);
    expect(screen.getByText("No departures yet.")).toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Stop" })).toBeNull();
  });
});

describe("the service alerts", () => {
  it("list every alert, narrow to one line, and say when a line has none", async () => {
    render(<Alerts alerts={alerts} />);
    const all = screen.getByRole("list", { name: "Service alerts" });
    expect(within(all).getAllByRole("listitem")).toHaveLength(2);
    const line = screen.getByRole("combobox", { name: "Line" });
    expect(within(line).getAllByRole("option").map((option) => option.textContent)).toEqual(["All lines", "100HE", "2550"]);
    await userEvent.selectOptions(line, "2550");
    expect(within(screen.getByRole("list", { name: "Service alerts" })).getAllByRole("listitem")).toHaveLength(1);
    expect(alertsFor(alerts, "4570")).toEqual([]);
    expect(alertsFor(alerts, undefined)).toBe(alerts);
  });

  it("render HSL's text as text, never as HTML", () => {
    const { container } = render(<Alerts alerts={alerts} />);
    expect(screen.getByText("<b>Detour</b>")).toBeInTheDocument();
    expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(container.querySelector("b, img")).toBeNull();
  });

  it("say so when there are none", () => {
    render(<Alerts alerts={[]} />);
    expect(screen.getByText("No service alerts.")).toBeInTheDocument();
  });

  it("mark the stops they name on the map, one point per placed stop", async () => {
    expect(alertPlaces(alerts).features).toEqual([{
      type: "Feature", id: "urn:ngsi-ld:Alert:hel.fi:helsinki:hsl-1#1392501",
      geometry: { type: "Point", coordinates: [25.03, 60.27] },
      properties: { alert: "urn:ngsi-ld:Alert:hel.fi:helsinki:hsl-1", stop: "1392501", title: "Stop closed" },
    }]);
    render(<App />);
    await waitFor(() =>
      expect(setData.mock.calls.some(([data]) => (data as { features: Array<{ properties: { stop?: string } }> }).features.some((f) => f.properties.stop === "1392501"))).toBe(true),
    );
    expect(await screen.findByText("Stop closed")).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "Departures from Tapiola" })).toBeInTheDocument();
  });
});

describe("the backend's answers", () => {
  it("are empty before its first read, and a malformed one is left out", async () => {
    backend({ "api/departures": [{ id: 7 }, ...stops], "api/alerts": { not: "a list" } });
    expect(await getDepartures()).toEqual(stops);
    expect(await getAlerts()).toEqual([]);
    vi.stubGlobal("fetch", vi.fn(async () => new Response("", { status: 503 })));
    expect(await getDepartures()).toEqual([]);
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("offline"); }));
    expect(await getAlerts()).toEqual([]);
  });

  it("are read again every half minute, and a failed read keeps the departures shown", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    render(<App />);
    await screen.findByRole("list", { name: "Departures from Tapiola" });
    vi.stubGlobal("fetch", vi.fn(async () => new Response("", { status: 502 })));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    expect(screen.getByRole("list", { name: "Departures from Tapiola" })).toBeInTheDocument();
    expect(fetch).toHaveBeenCalled();
  });
});
