// The explorer's map (T-3256): a located type's view drawn as clustered points read through the
// person's endpoint with the view's filters, a popup of text only with a button to the entity,
// and a failed read said with Retry.
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Bbox, MapLayer } from "../src/components/dashboards/MapLibreView";
import { colourOf, ExploreMap, isLocated, mapFeatures } from "../src/pages/explore/ExploreMap";

const drawn = vi.hoisted(() => ({ layers: [] as MapLayer[], onMoveEnd: undefined as undefined | ((bbox: Bbox) => void) }));
vi.mock("../src/components/dashboards/MapLibreView", async (original) => ({
  ...(await original<typeof import("../src/components/dashboards/MapLibreView")>()),
  MapLibreView: ({ layers, label, onMoveEnd }: { layers: MapLayer[]; label: string; onMoveEnd?: (bbox: Bbox) => void }) => {
    drawn.layers = layers;
    drawn.onMoveEnd = onMoveEnd;
    return <div role="application" aria-label={label} />;
  },
}));

const STATIONS = [
  { id: "urn:ngsi-ld:BikeStation:1", type: "BikeStation", name: "Kamppi", bikes: 3, location: { type: "Point", coordinates: [24.93, 60.17] } },
  { id: "urn:ngsi-ld:BikeStation:2", type: "BikeStation", name: "Unplaced", bikes: 0 },
];

function stub(answer: () => Response) {
  const asked: URL[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      asked.push(new URL(input instanceof Request ? input.url : String(input), window.location.origin));
      return answer();
    }),
  );
  return asked;
}

function show(onOpen = vi.fn(), geo = ["location"], colorBy?: string) {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <ExploreMap
          project="helsinki"
          slug="bikes-ops"
          query={{ type: "BikeStation", q: "bikes>0;name==\"Kamppi\"" }}
          columns={["name", "bikes", "address", "status", "extra"]}
          geo={geo}
          colorBy={colorBy}
          onOpen={onOpen}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return onOpen;
}

describe("the explorer's map", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    drawn.layers = [];
    drawn.onMoveEnd = undefined;
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("knows a located type by a GeoProperty or a location slot", () => {
    expect(isLocated([{ name: "position", kind: "GeoProperty" }])).toBe(true);
    expect(isLocated([{ name: "location", kind: "Property" }])).toBe(true);
    expect(isLocated([{ name: "bikes", kind: "Property" }])).toBe(false);
  });

  it("draws the located entities only, each with its id and its plain attributes", () => {
    expect(mapFeatures(STATIONS, ["name", "bikes", "location"]).features).toEqual([
      { type: "Feature", geometry: STATIONS[0].location, properties: { id: STATIONS[0].id, name: "Kamppi", bikes: 3 } },
    ]);
  });

  it("reads the view with its filter and draws it clustered, four attributes in the popup", async () => {
    const asked = stub(() => new Response(JSON.stringify(STATIONS), { headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "2" } }));
    show();
    expect(await screen.findByText("1 of 2 on the map")).toBeInTheDocument();
    expect(asked[0].searchParams.get("q")).toBe('bikes>0;name=="Kamppi"');
    expect(asked[0].searchParams.get("attrs")).toBe("location,name,bikes,address,status");
    expect(drawn.layers[0]).toMatchObject({ style: "circle", cluster: true, popupProperties: ["name", "bikes", "address", "status"] });
  });

  // T-6442: "colour-coded by occupancy": every point drawn, a point without the number grey and
  // counted, never the ramp's low end.
  it("colours by a number over its own range and counts the points without it", async () => {
    const point = { type: "Point", coordinates: [24.9, 60.1] };
    const row = (id: string, bikes?: number) => ({ id: `urn:ngsi-ld:BikeStation:${id}`, type: "BikeStation", location: point, ...(bikes === undefined ? {} : { bikes }) });
    const asked = stub(() => new Response(JSON.stringify([row("a", 0), row("b", 8), row("c")]), { status: 200, headers: { "NGSILD-Results-Count": "3" } }));
    show(undefined, undefined, "bikes");
    await screen.findByRole("application");
    expect(asked[0]?.searchParams.get("attrs")).toContain("bikes");
    expect(drawn.layers[0]).toMatchObject({ cluster: false, colorBy: { property: "bikes", domain: [0, 8] } });
    expect(screen.getByRole("status").textContent).toContain("Colour shows Bikes from 0 to 8, yellow to red. Grey: no value (1).");
    expect(colourOf([{ properties: {} }], "bikes")).toEqual({ domain: undefined, unknown: 1 });
  });

  it("says a map of one value in words and draws no ramp for it", async () => {
    const row = (id: string, bikes?: number) => ({ id: `urn:ngsi-ld:BikeStation:${id}`, type: "BikeStation", location: { type: "Point", coordinates: [24.9, 60.1] }, ...(bikes === undefined ? {} : { bikes }) });
    stub(() => new Response(JSON.stringify([row("a", 0), row("b", 0), row("c")]), { status: 200, headers: { "NGSILD-Results-Count": "3" } }));
    show(undefined, undefined, "bikes");
    await screen.findByRole("application");
    expect(drawn.layers[0]).toMatchObject({ colorBy: { property: "bikes", domain: [0, 0] } });
    expect(screen.getByRole("status").textContent).toContain("All points show Bikes 0, so one colour. Grey: no value (1).");
    expect(screen.getByText("all the same: 0")).toBeInTheDocument();
    expect(document.querySelector("#explore-map-ramp")).toBeNull();
    expect(screen.queryByText("0 to 0")).toBeNull();
  });

  it("opens the entity from the popup", async () => {
    stub(() => new Response(JSON.stringify(STATIONS), { headers: { "Content-Type": "application/json" } }));
    const onOpen = show();
    await screen.findByRole("application", { name: "Map of BikeStation" });
    const { popupNode } = await vi.importActual<typeof import("../src/components/dashboards/MapLibreView")>("../src/components/dashboards/MapLibreView");
    const node = popupNode({ properties: { id: STATIONS[0].id, name: "Kamppi" } }, ["name"], drawn.layers[0].open);
    document.body.append(node);
    await userEvent.click(screen.getByRole("button", { name: en.explore.map.open }));
    expect(onOpen).toHaveBeenCalledWith(STATIONS[0].id);
    node.remove();
  });

  it("keeps an attribute's markup as text in the popup", async () => {
    const { popupNode } = await vi.importActual<typeof import("../src/components/dashboards/MapLibreView")>("../src/components/dashboards/MapLibreView");
    const node = popupNode({ properties: { id: "x", name: '<img src=x onerror="alert(1)">' } }, ["name"]);
    expect(node.querySelector("img")).toBeNull();
    expect(node.textContent).toContain('<img src=x onerror="alert(1)">');
  });

  it("says why the read failed and reads again on Retry", async () => {
    let calls = 0;
    stub(() => {
      calls += 1;
      return calls === 1
        ? new Response("", { status: 503, statusText: "Service Unavailable" })
        : new Response(JSON.stringify([]), { headers: { "Content-Type": "application/json" } });
    });
    show();
    await userEvent.click(await screen.findByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText(en.explore.map.none)).toBeInTheDocument();
    expect(screen.queryByRole("application")).toBeNull();
  });

  // T-5061: past MAP_MAX the map reads the part in view after each pan or zoom.
  const point = (i: number) => ({ id: `urn:ngsi-ld:BikeStation:${i}`, type: "BikeStation", name: `S${i}`, location: { type: "Point", coordinates: [24 + (i % 100) / 100, 60 + i / 20000] } });
  const json = (rows: unknown[], count?: number) =>
    new Response(JSON.stringify(rows), { headers: { "Content-Type": "application/json", ...(count === undefined ? {} : { "NGSILD-Results-Count": String(count) }) } });
  const many = (inView: (url: URL) => Response) =>
    stub(() => json([])) &&
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const request = input instanceof Request ? input : new Request(String(input));
        asked.push(request);
        const url = new URL(request.url, window.location.origin);
        if (url.searchParams.has("georel")) return inView(url);
        const offset = Number(url.searchParams.get("offset") ?? 0);
        const limit = Number(url.searchParams.get("limit"));
        return json(Array.from({ length: limit }, (_, i) => point(offset + i)), 12_000);
      }),
    );
  const asked: Request[] = [];
  beforeEach(() => {
    asked.length = 0;
  });

  it("past 5,000 reads the part in view as GeoJSON-within on each move, with no token", async () => {
    many(() => json([point(1), point(2), point(3)], 3));
    show(vi.fn(), ["position", "location"]);
    expect(await screen.findByText(/^5,000 of 12,000 on the map. More than 5,000 match/)).toBeInTheDocument();
    expect(drawn.onMoveEnd).toBeTypeOf("function");
    await act(async () => drawn.onMoveEnd?.([24.1, 60.1, 24.5, 60.3]));
    expect(await screen.findByText(/^3 in this view, 12,000 in all. More than 5,000 match/)).toBeInTheDocument();
    const view = new URL(asked.find((r) => r.url.includes("georel"))?.url ?? "", window.location.origin);
    expect(view.searchParams.get("georel")).toBe("within");
    expect(view.searchParams.get("geometry")).toBe("Polygon");
    expect(JSON.parse(view.searchParams.get("coordinates") ?? "null")).toEqual([[[24.1, 60.1], [24.5, 60.1], [24.5, 60.3], [24.1, 60.3], [24.1, 60.1]]]);
    expect(view.searchParams.get("geoproperty")).toBe("position");
    expect(view.searchParams.get("q")).toBe('bikes>0;name=="Kamppi"');
    expect(asked.every((r) => !r.headers.has("authorization"))).toBe(true);
    expect((drawn.layers[0].data as { features: unknown[] }).features).toHaveLength(3);
  });

  it("says when the part in view still holds more than 5,000, and when it holds none", async () => {
    let answer: (url: URL) => Response = (url) => json(Array.from({ length: Number(url.searchParams.get("limit")) }, (_, i) => point(i)), 9_000);
    many((url) => answer(url));
    show();
    await screen.findByText(/^5,000 of 12,000 on the map/);
    await act(async () => drawn.onMoveEnd?.([24, 60, 25, 61]));
    expect(await screen.findByText(new RegExp(`${en.explore.map.truncated.replace("{max}", "5,000").replace(/[.:]/g, "\\$&")}$`))).toBeInTheDocument();
    answer = () => json([], 0);
    await act(async () => drawn.onMoveEnd?.([10, 10, 11, 11]));
    expect(await screen.findByText(new RegExp(`^${en.explore.map.noneInView.replace(/\./g, "\\.")}`))).toBeInTheDocument();
  });

  it("keeps the map and says so when the part in view cannot be read", async () => {
    many(() => new Response("", { status: 502, statusText: "Bad Gateway" }));
    show();
    await screen.findByText(/^5,000 of 12,000 on the map/);
    await act(async () => drawn.onMoveEnd?.([24, 60, 25, 61]));
    expect(await screen.findByRole("alert")).toHaveTextContent("move the map to try again");
    expect((drawn.layers[0].data as { features: unknown[] }).features).toHaveLength(5_000);
  });

  it("reads nothing more on a move while the whole view fits", async () => {
    stub(() => json(STATIONS, 2));
    show();
    await screen.findByText("1 of 2 on the map");
    expect(drawn.onMoveEnd).toBeUndefined();
  });
});
