/**
 * The chart and the map (T-2925, AP-56): one day of PM10 and PM2.5 per station with each EU
 * limit, and the stations on a map coloured by their air quality index band; a click on the map
 * picks the station the chart is about.
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "../src/App";
import { Chart } from "../src/Chart";
import { basemapOf, StationMap } from "../src/StationMap";
import { BAND_COLOUR, bandOf, historyOf, LIMITS, stationFeatures, type StationCollection } from "../src/quality";
import type { Station } from "../src/api";
import { Map as FakeMap } from "./maplibre";

const KALLIO = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:kallio";
const KUMPULA = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:kumpula";
const LOST = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:lost";

const stations: Station[] = [
  { id: KALLIO, name: "Kallio", pm10: 34.2, pm25: 21, airQualityIndex: 3.4, coordinates: [24.95, 60.18] },
  { id: KUMPULA, name: "Kumpula", pm10: 8, airQualityIndex: 1, coordinates: [24.96, 60.2] },
  // A station a steward added without a position: listed, never drawn at 0,0.
  { id: LOST, name: "Lost", own: true },
];

const kallioDay = {
  id: KALLIO,
  pm10: [
    { type: "Property", value: 57.5, observedAt: "2026-09-06T09:00:00Z" },
    { type: "Property", value: 18, observedAt: "2026-09-06T08:00:00Z" },
  ],
  pm25: { type: "Property", value: 21, observedAt: "2026-09-06T10:00:00Z" },
};

function serve(history: (url: string) => Response) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith("api/me")) {
        return Promise.resolve(new Response(JSON.stringify({ signedIn: false, email: null, user: null, anonymous: true, roles: [] })));
      }
      if (url.endsWith("api/stations")) return Promise.resolve(new Response(JSON.stringify(stations)));
      return Promise.resolve(history(url));
    }),
  );
}

afterEach(() => {
  vi.restoreAllMocks();
  FakeMap.built.length = 0;
});

describe("what a reading means", () => {
  it("bands the 1…5 index to the nearest step and never calls a missing index good", () => {
    expect([1, 2.4, 2.5, 4, 5.6].map(bandOf)).toEqual(["good", "satisfactory", "fair", "poor", "veryPoor"]);
    expect(bandOf(undefined)).toBe("unknown");
    expect(bandOf(Number.NaN)).toBe("unknown");
    expect(bandOf(0)).toBe("good");
  });

  it("reads a day in time order, a single instance too, and drops what has no number or no time", () => {
    const day = historyOf({
      pm10: [
        { value: 30, observedAt: "2026-09-06T10:00:00Z" },
        { value: 20, observedAt: "2026-09-06T09:00:00Z" },
        { value: "high", observedAt: "2026-09-06T11:00:00Z" },
        { value: 40 },
      ],
      pm25: { value: 12, observedAt: "2026-09-06T09:00:00Z" },
    });
    expect(day.pm10.map((point) => point.value)).toEqual([20, 30]);
    expect(day.pm25.map((point) => point.value)).toEqual([12]);
    expect(historyOf(null)).toEqual({ pm10: [], pm25: [] });
  });

  it("puts one feature on the map per station with a position, in its band's colour", () => {
    const collection = stationFeatures(stations);
    expect(collection.features.map((feature) => feature.properties.id)).toEqual([KALLIO, KUMPULA]);
    expect(collection.features[0].properties.colour).toBe(BAND_COLOUR.fair);
    expect(collection.features[1].properties.colour).toBe(BAND_COLOUR.good);
  });
});

describe("the basemap", () => {
  it("is the project's style from the page's #jc-config, and nothing when the page has none or it is broken", () => {
    const page = (text: string | null) => {
      const doc = document.implementation.createHTMLDocument("app");
      if (text !== null) {
        const script = doc.createElement("script");
        script.id = "jc-config";
        script.type = "application/json";
        script.textContent = text;
        doc.body.append(script);
      }
      return doc;
    };
    expect(basemapOf(page('{"basemap":"https://portal.example/api/v1/projects/helsinki/basemap/style.json"}'))).toBe(
      "https://portal.example/api/v1/projects/helsinki/basemap/style.json",
    );
    expect(basemapOf(page(null))).toBeUndefined();
    expect(basemapOf(page("{not json"))).toBeUndefined();
    expect(basemapOf(page('{"basemap":42}'))).toBeUndefined();
  });
});

describe("the chart", () => {
  it("draws one line per pollutant and each pollutant's limit, and says the latest values in words", () => {
    const { container } = render(<Chart history={historyOf(kallioDay)} station="Kallio" />);
    expect(container.querySelectorAll("polyline")).toHaveLength(2);
    expect(container.querySelectorAll("line.limit")).toHaveLength(2);
    const figure = screen.getByRole("img");
    expect(figure).toHaveAccessibleName(
      `PM readings at Kallio over the last 24 hours. Latest: PM10 57.5 µg/m³ (limit ${LIMITS.pm10}), PM2.5 21 µg/m³ (limit ${LIMITS.pm25}).`,
    );
    // The series take the token palette, never a colour from the data.
    const [pm10, pm25] = [...container.querySelectorAll("polyline")].map((line) => line.getAttribute("stroke"));
    expect(pm10).not.toBe(pm25);
  });

  it("puts a single reading in the middle rather than dividing by a zero-length day", () => {
    const { container } = render(<Chart history={historyOf({ pm10: { value: 12, observedAt: "2026-09-06T10:00:00Z" } })} station="Kumpula" />);
    const [line] = [...container.querySelectorAll("polyline")];
    expect(line.getAttribute("points")).not.toMatch(/NaN/);
  });

  it("says a station sent nothing today rather than drawing an empty axis", () => {
    render(<Chart history={historyOf({})} station="Kumpula" />);
    expect(screen.getByRole("status")).toHaveTextContent("No PM10 or PM2.5 readings from Kumpula in the last 24 hours.");
    expect(screen.queryByRole("img")).toBeNull();
  });
});

describe("the page", () => {
  it("draws the located stations on the map and a click on one shows that station's day", async () => {
    serve((url) =>
      new Response(JSON.stringify(url.includes(encodeURIComponent(KALLIO)) ? kallioDay : { id: KUMPULA })),
    );
    render(<App />);
    expect(await screen.findByRole("img", { name: /at Kallio/ })).toBeInTheDocument();

    // The map is built after the chart shows, a beat later on a loaded runner (T-2994).
    const map = await waitFor(() => {
      const [built] = FakeMap.built;
      expect((built?.sources.stations?.data as StationCollection | undefined)?.features).toHaveLength(2);
      return built!;
    });
    const drawn = map.sources.stations.data as StationCollection;
    expect(drawn.features).toHaveLength(2);
    expect(map.layers[0]).toMatchObject({ type: "circle", paint: { "circle-color": ["get", "colour"] } });

    act(() => map.fire("click", { features: [{ properties: { id: KUMPULA } }] }, "stations"));
    expect(await screen.findByText("No PM10 or PM2.5 readings from Kumpula in the last 24 hours.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Kumpula/ })).toHaveAttribute("aria-pressed", "true");
    // The legend says every band in words beside its colour (UI-30).
    const legend = screen.getByRole("list", { name: "Air quality index" });
    expect(within(legend).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "Good",
      "Satisfactory",
      "Fair",
      "Poor",
      "Very poor",
    ]);
  });

  it("picks a station from the list for the chart and opens it in the panel, one without a position too", async () => {
    serve((url) => new Response(JSON.stringify(url.includes(encodeURIComponent(KALLIO)) ? kallioDay : { id: KUMPULA })));
    render(<App />);
    const list = await screen.findByRole("list", { name: "Stations" });
    fireEvent.click(within(list).getByRole("button", { name: "Kumpula: Good" }));
    expect(within(list).getByRole("button", { name: "Kumpula: Good" })).toHaveAttribute("aria-pressed", "true");
    expect(await screen.findByRole("dialog", { name: "Kumpula" })).toBeInTheDocument();
    fireEvent.click(within(list).getByRole("button", { name: "Kallio: Fair" }));
    expect(await screen.findByRole("dialog", { name: "Kallio" })).toBeInTheDocument();
    fireEvent.click(within(list).getByRole("button", { name: "Lost: No index" }));
    expect(await screen.findByRole("dialog", { name: "Lost" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    fireEvent.click(screen.getByRole("button", { name: "Details of Lost" }));
    expect(await screen.findByRole("dialog", { name: "Lost" })).toBeInTheDocument();
    for (const name of ["Details of Kallio", "Details of Kumpula"]) fireEvent.click(screen.getByRole("button", { name }));
  });

  it("centres on Helsinki and says so when no station has a position, and names a station by its id when it has no name", async () => {
    const onSelect = vi.fn();
    render(<StationMap stations={[{ id: LOST }]} selected={null} onSelect={onSelect} />);
    expect(screen.getByRole("status")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: `${LOST}: No index` }));
    expect(onSelect).toHaveBeenCalledWith(LOST);
    const [map] = FakeMap.built;
    expect(map.options.center).toEqual([24.94, 60.17]);
    // A click on the map beside every station, or on a feature without an id, picks nothing.
    act(() => map.fire("click", { features: [] }, "stations"));
    act(() => map.fire("click", { features: [{ properties: { id: 7 } }] }, "stations"));
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("shows the gateway's own words when the day cannot be read", async () => {
    serve(() => new Response(JSON.stringify({ detail: "queryTemporal is outside this app's window" }), { status: 403 }));
    render(<App />);
    expect(await screen.findByRole("alert")).toHaveTextContent("queryTemporal is outside this app's window");
  });
});
