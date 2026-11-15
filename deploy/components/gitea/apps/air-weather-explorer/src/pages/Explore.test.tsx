import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { AIR_STATIONS, NOW, WEATHER_STATIONS } from "../fixtures/stations";
import App from "../App";
import { ServerProblem } from "../server";
import { server } from "../testing/server";
import { choiceOf, PERIODS, sentence } from "./Explore";

// The App's server, in memory (T-3348).
vi.mock("../server", async (original) => {
  const { server } = await import("../testing/server");
  return { ...(await original<typeof import("../server")>()), airApi: () => server };
});
vi.mock("maplibre-gl", () => import("../testing/maplibre"));

/** Each chart the page drew: its canvas, the option it was last given, and its click handler. */
const charts = vi.hoisted(() => [] as Array<{ el: HTMLElement; option: Record<string, unknown> | null; click: ((params: unknown) => void) | null }>);
vi.mock("echarts/core", () => ({
  use: () => undefined,
  init: (el: HTMLElement) => {
    const chart = { el, option: null as Record<string, unknown> | null, click: null as ((params: unknown) => void) | null };
    charts.push(chart);
    return {
      setOption: (option: Record<string, unknown>) => {
        chart.option = option;
      },
      resize: () => undefined,
      dispose: () => undefined,
      on: (_event: string, handler: (params: unknown) => void) => {
        chart.click = handler;
      },
    };
  },
}));

/** The chart now on the page under `title`. */
function chart(title: string) {
  const found = [...charts].reverse().find((c) => c.el.isConnected && c.el.getAttribute("aria-label") === title);
  if (!found) throw new Error(`no chart "${title}"`);
  return found;
}

const READ = {
  permissions: [
    { resource: { type: "AirQualityObserved" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const },
    { resource: { type: "WeatherObserved" }, actions: ["queryEntity", "retrieveEntity", "retrieveTemporal"], attributes: "*" as const },
  ],
  prohibitions: [],
};

function show(language = "en", fixture: Parameters<typeof stubClient>[0] = {}) {
  const client = stubClient({ entities: [...AIR_STATIONS, ...WEATHER_STATIONS], access: READ, ...fixture }, { appName: "air-weather-explorer", language });
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

beforeEach(() => {
  server.reset();
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date(NOW));
  window.history.replaceState(null, "", "/");
});
afterEach(() => {
  vi.useRealTimers();
  window.history.replaceState(null, "", "/");
  charts.length = 0;
});

// T-3330: the first screen answers "how does the weather move the air here" without a click.
describe("Explore", () => {
  it("compares the first station with the nearest weather station and says what moves it", async () => {
    const client = show();
    const answer = screen.getByRole("region", { name: "The answer" });
    await waitFor(() => expect(answer).toHaveTextContent("Kallio 2: PM2.5 (µg/m³) falls as wind speed (m/s) rises, strongly."));
    expect(answer).toHaveTextContent(/Spearman −\d,\d\d, Pearson −\d,\d\d, over 72 common hours/);
    expect(screen.getByRole("combobox", { name: "Weather station" })).toHaveDisplayValue(/Helsinki Kaisaniemi \(1,\d km\)/);
    // The history comes from the App's server, which keeps it; the page reads only the stations.
    expect(server.series).toHaveBeenCalledWith(AIR_STATIONS[0].id, WEATHER_STATIONS[0].id, 3);
    expect(client.transport.calls.some((call) => call.path.includes("/temporal/"))).toBe(false);
    expect(client.transport.calls.every((call) => call.method === "GET" && call.path.includes("/api/endpoint/"))).toBe(true);
  });

  it("lists every pair, marks the outlying reading, and keeps a chosen pair in the address", async () => {
    show();
    const table = await screen.findByRole("table", { name: "Every pair" });
    expect(within(table).getAllByRole("row")).toHaveLength(1 + 3 * 3);
    expect(screen.getByText(/1 outlying readings \(PM2.5/)).toBeInTheDocument();
    fireEvent.click(within(table).getByRole("button", { name: "PM10 (µg/m³) · temperature (°C)" }));
    expect(new URLSearchParams(window.location.search).get("air")).toBe("pm10");
    expect(new URLSearchParams(window.location.search).get("w")).toBe("temperature");
    await waitFor(() => expect(screen.getByRole("region", { name: "The answer" })).toHaveTextContent("PM10 (µg/m³)"));
    // Every row picks its own pair, and the chart's title follows.
    for (const button of within(table).getAllByRole("button")) {
      fireEvent.click(button);
      const [air, weather] = (button.textContent ?? "").split(" · ");
      expect(await screen.findByRole("img", { name: `${air} and ${weather} by the hour` })).toBeInTheDocument();
    }
  });

  it("says there is nothing to compare for a station with no history", async () => {
    window.history.replaceState(null, "", "/?station=vartiokyla");
    show();
    expect(await screen.findByText("No measurements in the chosen period.", { selector: ".app-answer *" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Details of Vartiokylä" }));
    expect(await screen.findByRole("dialog", { name: "Vartiokylä" })).toBeInTheDocument();
  });

  it("says too few hours rather than a coefficient, and draws nothing for a pair with no reading", async () => {
    const at = (h: number) => new Date(NOW - h * 3_600_000).toISOString();
    const short = [
      { id: AIR_STATIONS[0].id, type: "AirQualityObserved", pm25: { type: "Property", values: [[10, at(3)], [12, at(2)]] }, pm10: { type: "Property", values: [[20, at(30)]] } },
      { id: WEATHER_STATIONS[0].id, type: "WeatherObserved", windSpeed: { type: "Property", values: [[3, at(3)], [4, at(2)]] }, temperature: { type: "Property", values: [[5, at(50)]] } },
    ];
    window.history.replaceState(null, "", "/?air=pm10&w=temperature");
    server.readings = short;
    show();
    expect(await screen.findByText("Too few common hours to say anything. Pick a longer period or another station.", { selector: ".app-answer *" })).toBeInTheDocument();
    const table = screen.getByRole("table", { name: "Every pair" });
    expect(within(table).getAllByRole("cell").some((cell) => cell.textContent === "–")).toBe(true);
  });

  it("speaks Finnish when the Portal does, every pair and every filter too", async () => {
    show("fi");
    const answer = screen.getByRole("region", { name: "Vastaus" });
    await waitFor(() => expect(answer).toHaveTextContent("PM2.5 (µg/m³) laskee, kun tuulen nopeus (m/s) nousee, voimakkaasti."));
    const table = await screen.findByRole("table", { name: "Kaikki parit" });
    for (const button of within(table).getAllByRole("button")) {
      fireEvent.click(button);
      const [air, weather] = (button.textContent ?? "").split(" · ");
      await waitFor(() => expect(screen.getByRole("combobox", { name: "Saaste" })).toHaveDisplayValue(air));
      expect(screen.getByRole("combobox", { name: "Säämuuttuja" })).toHaveDisplayValue(weather);
    }
    fireEvent.change(screen.getByRole("combobox", { name: "Saaste" }), { target: { value: "pm25" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Säämuuttuja" }), { target: { value: "windSpeed" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Ajanjakso" }), { target: { value: "1" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Liukuva keskiarvo" }), { target: { value: "1" } });
    expect(new URLSearchParams(window.location.search).get("days")).toBe("1");
    fireEvent.change(screen.getByRole("combobox", { name: "Ilmanlaatuasema" }), { target: { value: "kallio" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Sääasema" }), { target: { value: "fmi-100971" } });
    fireEvent.click(screen.getByRole("button", { name: "Tiedot: Kallio 2" }));
    expect(await screen.findByRole("dialog", { name: "Kallio 2" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Tiedot: Helsinki Kaisaniemi" }));
    const panel = await screen.findByRole("dialog", { name: "Helsinki Kaisaniemi" });
    fireEvent.click(within(panel).getByRole("button", { name: "Sulje" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // The language already shown is no reason to reload.
    const language = screen.getByRole("combobox", { name: "Kieli" });
    fireEvent.change(language, { target: { value: "fi" } });
    expect(language).toHaveValue("fi");
    expect(new URLSearchParams(window.location.search).get("lang")).toBeNull();
  });

  // T-3348: a saved comparison opened from its link.
  it("opens a saved comparison's choices from its link and drops the code from the address", async () => {
    const saved = await server.save({ name: "Kallio and the wind", station: AIR_STATIONS[0].id, weather: WEATHER_STATIONS[0].id, days: 7, smoothing: 6, air: "pm10", variable: "temperature" });
    window.history.replaceState(null, "", `/?compare=${saved.code}`);
    show();
    expect(await screen.findByText("Saved comparison opened: Kallio and the wind.")).toBeInTheDocument();
    const params = new URLSearchParams(window.location.search);
    expect(params.get("compare")).toBeNull();
    expect([params.get("station"), params.get("weather"), params.get("days"), params.get("window"), params.get("air"), params.get("w")]).toEqual(["kallio", "fmi-100971", "7", "6", "pm10", "temperature"]);
    await waitFor(() => expect(server.series).toHaveBeenCalledWith(AIR_STATIONS[0].id, WEATHER_STATIONS[0].id, 7));
  });

  it("says so when a comparison link names nothing, and never asks for a code it could not have made", async () => {
    window.history.replaceState(null, "", "/?compare=zzzzzzzzzzzz");
    show();
    expect(await screen.findByRole("alert")).toHaveTextContent("No comparison is saved under this link.");
    cleanup();
    window.history.replaceState(null, "", "/?compare=..%2Fx");
    server.reset();
    show();
    expect(await screen.findByText("No comparison is saved under this link.")).toBeInTheDocument();
    expect(server.open).not.toHaveBeenCalled();
  });

  it("says so when there is no station", async () => {
    show("en", { entities: [] });
    expect(await screen.findByText("No air quality station was found.")).toBeInTheDocument();
  });

  it("says in words when the history cannot be read, and offers to try again", async () => {
    server.series = vi.fn(async () => {
      throw new ServerProblem(503, "the gateway could not answer right now; try again shortly");
    });
    show();
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The measurements could not be read (HTTP 503). Try again later.");
    expect(server.series).toHaveBeenCalledTimes(1);
    fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(server.series).toHaveBeenCalledTimes(2));
    expect(await screen.findByRole("alert")).toHaveTextContent("HTTP 503");
  });

  it("says the connection is down when nothing answered at all", async () => {
    server.series = vi.fn(async () => {
      throw new ServerProblem(0, "");
    });
    show();
    expect(await screen.findByRole("alert")).toHaveTextContent("The measurements could not be read. Check the connection and try again.");
  });

  it("changes the comparison with every filter, keeps each in the address, and opens each chosen station", async () => {
    show();
    const answer = screen.getByRole("region", { name: "The answer" });
    await waitFor(() => expect(answer).toHaveTextContent("Kallio 2:"));
    const filters: Array<[string, string, string]> = [
      ["Period", "7", "days"],
      ["Rolling mean", "6", "window"],
      ["Pollutant", "pm10", "air"],
      ["Weather variable", "temperature", "w"],
    ];
    for (const [name, value, param] of filters) {
      // A new period reads the history again; the pair's lists wait for its analysis.
      await waitFor(() => expect(screen.getByRole("combobox", { name })).toBeEnabled());
      fireEvent.change(screen.getByRole("combobox", { name }), { target: { value } });
      expect(new URLSearchParams(window.location.search).get(param)).toBe(value);
    }
    await waitFor(() => expect(answer).toHaveTextContent("PM10 (µg/m³)"));
    expect(answer).toHaveTextContent("temperature (°C)");

    fireEvent.click(screen.getByRole("button", { name: "Details of Kallio 2" }));
    expect(await screen.findByRole("dialog", { name: "Kallio 2" })).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "Weather station" }), { target: { value: "road-1002" } });
    fireEvent.click(screen.getByRole("button", { name: "Details of kt51_Hki_Lapinlahti" }));
    expect(await screen.findByRole("dialog", { name: "kt51_Hki_Lapinlahti" })).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "Air quality station" }), { target: { value: "makelankatu" } });
    fireEvent.click(screen.getByRole("button", { name: "Details of Mäkelänkatu" }));
    expect(await screen.findByRole("dialog", { name: "Mäkelänkatu" })).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("station")).toBe("makelankatu");
  });

  it("picks a pair from the correlation matrix and labels its cells with the coefficient", async () => {
    show();
    await screen.findByRole("table", { name: "Every pair" });
    const matrix = await waitFor(() => chart("Correlations (Spearman)"));
    const series = (matrix.option?.series as Array<{ label: { formatter(p: { value: [number, number, number] }): string } }>)[0];
    expect(series.label.formatter({ value: [0, 0, -0.456] })).toBe("−0,46");
    act(() => matrix.click?.({ name: "pm10|relativeHumidity" }));
    expect(new URLSearchParams(window.location.search).get("w")).toBe("relativeHumidity");
    act(() => matrix.click?.({ name: "nonsense" }));
    act(() => matrix.click?.({}));
    expect(new URLSearchParams(window.location.search).get("air")).toBe("pm10");
    await waitFor(() => expect(screen.getByRole("region", { name: "The answer" })).toHaveTextContent("PM10 (µg/m³)"));
  });
});

describe("the page's pure parts", () => {
  it("takes a period only from the allowed ones", () => {
    expect(choiceOf("7", PERIODS, 3)).toBe(7);
    for (const wrong of ["", "2", "30", "x"]) expect(choiceOf(wrong, PERIODS, 3)).toBe(3);
  });

  it("says nothing of a pair without a correlation, and hardly of a weak one", () => {
    expect(sentence({ air: "pm10", weather: "windSpeed", n: 3, pearson: null, spearman: null }, "en")).toBeNull();
    expect(sentence({ air: "pm10", weather: "windSpeed", n: 30, pearson: 0.05, spearman: 0.1 }, "en")?.claim).toBe("PM10 (µg/m³) hardly moves with wind speed (m/s)");
  });
});
