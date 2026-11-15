import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { JcProvider, ProblemError } from "@joinedcontext/sdk";
import type { TemporalRow } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "../App";
import { EstimaterContext } from "../estimate";
import type { Estimater } from "../estimate";
import type { EstimateOutput } from "../bikes";
import { ENTITIES, NOW, STATION_1, STATION_2, STATION_3, TEMPORAL, WEATHER_1 } from "../fixtures/bikes";
import { ServerContext } from "../server";
import type { Server } from "../server";
import { memory } from "../testing/server";

/** The App's server with no model and no inputs kept yet, signed out (T-3355, T-3827);
 * ModelHistory.test.tsx and roles.test.tsx test those parts themselves. */
const NO_MODELS: Server = memory();

vi.mock("maplibre-gl", () => ({
  Map: class {
    on = vi.fn();
    once = vi.fn();
    remove = vi.fn();
  },
  setWorkerUrl: vi.fn(),
}));
vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

const ACCESS = {
  permissions: [
    { resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const },
    { resource: { type: "WeatherObserved" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const },
  ],
  prohibitions: [],
};

/** An estimate the page is handed as it is, to say each of its cases in words. */
const OUTPUT: EstimateOutput = {
  hours: 30,
  profile: [
    { slot: 0, mean: 5, count: 2 },
    { slot: 1, mean: 5, count: 2 },
    { slot: 2, mean: null, count: 0 },
  ],
  weather: { perDegree: -0.4, rain: 1.5, hours: 30 },
  sigma: 1,
  estimate: [{ at: "not a time", mean: 1, low: 0, high: 2 }],
  assumption: { temperature: 12, raining: true },
  enough: true,
};

function client(entities = ENTITIES, temporal: (type: string) => Promise<TemporalRow[]> | null = () => null) {
  const made = stubClient({ entities, temporal: TEMPORAL, access: ACCESS }, { appName: "bike-weather-demand" });
  const original = made.temporal.list.bind(made.temporal);
  made.temporal.list = vi.fn((type, query) => temporal(type) ?? original(type, query));
  return made;
}

function show(estimater: Estimater, made = client()) {
  render(
    <JcProvider client={made}>
      <ServerContext.Provider value={NO_MODELS}>
        <EstimaterContext.Provider value={estimater}>
          <App />
        </EstimaterContext.Provider>
      </ServerContext.Provider>
    </JcProvider>,
  );
  return made;
}

beforeEach(() => {
  vi.setSystemTime(new Date(NOW * 1000));
  window.history.replaceState(null, "", "/?lang=en");
});
afterEach(() => {
  vi.useRealTimers();
  window.history.replaceState(null, "", "/");
});

describe("the station page in words", () => {
  it("says a weather effect that lowers the count, rain that raises it, a rainy whole-degree assumption, and an unmeasured hour", async () => {
    show(async () => OUTPUT);
    expect(await screen.findByText(/1 °C warmer: −0\.4 bikes; rain: \+1\.5 bikes; from 30 hours with weather/)).toBeInTheDocument();
    expect(screen.getByText(/12 °C, rain$/)).toBeInTheDocument();
    const table = screen.getByRole("table", { name: "Availability by hour of the week" });
    expect(within(table).getAllByText("5")).toHaveLength(2);
    expect(within(table).getAllByText("—").length).toBeGreaterThan(100);
  });

  it("says too little history, and no weather terms where no weather station is near", async () => {
    show(async () => ({ ...OUTPUT, enough: false }));
    expect(await screen.findByText(/Too little history/)).toBeInTheDocument();
    cleanupAndShow(async () => ({ ...OUTPUT, weather: null, assumption: null }), client([STATION_1, STATION_2, STATION_3]));
    expect(await screen.findByText(/No weather station within 10 km/)).toBeInTheDocument();
  });

  it("says a failed estimate in words, whatever was thrown", async () => {
    show(async () => {
      throw new Error("boom");
    });
    expect(await screen.findByText("Estimation failed: boom")).toBeInTheDocument();
    cleanupAndShow(() => Promise.reject("bare"));
    expect(await screen.findByText("Estimation failed: bare")).toBeInTheDocument();
  });

  it("reads history values written as text, skips ones that are no number, and draws nothing from none", async () => {
    const estimater = vi.fn(async () => OUTPUT);
    show(
      estimater,
      client(ENTITIES, (type) =>
        type === "BikeHireDockingStation"
          ? Promise.resolve([
              { id: STATION_1.id, type, series: { availableBikeNumber: [{ observedAt: "2026-10-01T10:00:00Z", value: "7" }, { observedAt: "2026-10-01T11:00:00Z", value: "x" }] } },
            ] as unknown as TemporalRow[])
          : Promise.resolve([
              {
                id: WEATHER_1.id,
                type,
                series: {
                  temperature: [{ observedAt: "2026-10-01T10:00:00Z", value: "11.5" }, { observedAt: "2026-10-01T11:00:00Z", value: "warm" }],
                  precipitation: [{ observedAt: "2026-10-01T12:00:00Z", value: "x" }, { observedAt: "2026-10-01T10:00:00Z", value: 0.4 }],
                },
              },
            ] as unknown as TemporalRow[]),
      ),
    );
    await waitFor(() => expect(estimater).toHaveBeenCalled());
    const input = (estimater.mock.calls.at(-1) as unknown as [{ bikes: unknown[]; weather: Array<{ at: string; temperature: number | null; precipitation: number | null }> }])[0];
    expect(input.weather).toEqual([
      { at: "2026-10-01T10:00:00Z", temperature: 11.5, precipitation: 0.4 },
      { at: "2026-10-01T11:00:00Z", temperature: null, precipitation: null },
      { at: "2026-10-01T12:00:00Z", temperature: null, precipitation: null },
    ]);
  });

  it("reads a weather station that answers no series as no weather", async () => {
    const estimater = vi.fn(async () => OUTPUT);
    show(estimater, client(ENTITIES, (type) => (type === "WeatherObserved" ? Promise.resolve([{ id: WEATHER_1.id, type, series: {} }] as unknown as TemporalRow[]) : null)));
    await waitFor(() => expect(estimater).toHaveBeenCalled());
    expect((estimater.mock.calls.at(-1) as unknown as [{ weather: unknown[] }])[0].weather).toEqual([]);
  });

  it("says a history that could not be read, and asks again on Retry", async () => {
    let fail: unknown = new ProblemError(503, { title: "History unavailable", status: 503 });
    const made = show(async () => OUTPUT, client(ENTITIES, (type) => (type === "BikeHireDockingStation" && fail ? Promise.reject(fail) : null)));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("History unavailable");
    fail = null;
    fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(made.temporal.list).toHaveBeenCalledTimes(4));
    cleanupAndShow(async () => OUTPUT, client(ENTITIES, (type) => (type === "BikeHireDockingStation" ? Promise.reject("down") : null)));
    expect(await screen.findByRole("alert")).toHaveTextContent("down");
  });

  it("follows a station named in the address after the page opened", async () => {
    show(async () => OUTPUT);
    const list = await screen.findByRole("combobox", { name: "Station" });
    window.location.hash = `#station?id=${encodeURIComponent(STATION_3.id)}`;
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    await waitFor(() => expect(list).toHaveValue(STATION_3.id));
    // The same station again, or none, changes nothing.
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    window.location.hash = "";
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    expect(list).toHaveValue(STATION_3.id);
  });

  it("keeps a station the address named while the stations were arriving, over the default", async () => {
    // The address names a station in the moment the stations are on screen and the page's effects
    // have not run yet (a slow CI runner hit it, run 39343158555): the default must not win.
    const observer = new MutationObserver(() => {
      if (!document.querySelector(`option[value="${STATION_3.id}"]`)) return;
      observer.disconnect();
      window.location.hash = `#station?id=${encodeURIComponent(STATION_3.id)}`;
      window.dispatchEvent(new HashChangeEvent("hashchange"));
    });
    observer.observe(document.body, { childList: true, subtree: true });
    show(async () => OUTPUT);
    const list = await screen.findByRole("combobox", { name: "Station" });
    await waitFor(() => expect(list).toHaveValue(STATION_3.id));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(list).toHaveValue(STATION_3.id);
    expect(window.location.hash).toBe(`#station?id=${encodeURIComponent(STATION_3.id)}`);
  });

  it("falls back to the default when the address names a station the list does not hold", async () => {
    show(async () => OUTPUT);
    const list = await screen.findByRole("combobox", { name: "Station" });
    await waitFor(() => expect(list).not.toHaveValue(""));
    const first = (list as HTMLSelectElement).value;
    window.location.hash = "#station?id=urn%3Angsi-ld%3ABikeHireDockingStation%3Agone";
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    await waitFor(() => expect(window.location.hash).toBe(`#station?id=${encodeURIComponent(first)}`));
    expect(list).toHaveValue(first);
  });
});

/** Unmounts what is on screen and shows the page again, for a second case in one test. */
function cleanupAndShow(estimater: Estimater, made = client()) {
  document.body.innerHTML = "";
  show(estimater, made);
}
