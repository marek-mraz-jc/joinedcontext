/**
 * The district map of the region's indicators (T-2933, AP-04, AP-67, AP-123).
 *
 * `fetch` is stubbed as in `App.test.tsx`, and MapLibre is replaced by a recorder, so what is read
 * here is what the page asks the endpoints for and what it hands the map: one shape per district,
 * the indicator's number on each, the range the tokens' ramp spans, and the district picked.
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider, mapColors } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import region from "./fixtures/bbsk-kpi.json";
import city from "./fixtures/banskabystrica-kpi.json";
import { rampColor } from "./districts";

type Collection = { features: Array<{ id: string; properties: Record<string, unknown> }> };
const drawn: Collection[] = [];
const paints: unknown[] = [];
const styles: unknown[] = [];
const clicks: Record<string, (event: unknown) => void> = {};

vi.mock("maplibre-gl", () => {
  class Map {
    constructor(options: { style?: unknown }) {
      styles.push(options.style);
    }
    on(event: string, layerOrHandler: unknown, handler?: (event: unknown) => void) {
      if (event === "load" && typeof layerOrHandler === "function") (layerOrHandler as () => void)();
      if (event === "click" && typeof layerOrHandler === "string" && handler) clicks[layerOrHandler] = handler;
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    getSource = () => ({ setData: (data: Collection) => drawn.push(data) });
    setPaintProperty = (_layer: string, _name: string, value: unknown) => paints.push(value);
    fitBounds = vi.fn();
    remove = vi.fn();
  }
  return { Map, setWorkerUrl: vi.fn() };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const App = (await import("./App")).default;

const REGION_SLUG = "7u4ns3cdg2mqlx5gmxhk7rqai6pmokdj";
const CITY_SLUG = "qfhhh5no5wz4lk3rfjisdtx3chiyfig3";
const REGISTER_SLUG = "lws6dd7o56veegwxsjtvak3goeta4mfn";
const BASEMAP = "https://portal.example/api/v1/projects/bbsk/basemap/default/style.json";

const ENDPOINTS = [
  { name: "bbsk-kpi", slug: REGION_SLUG, space: "bbsk-kpi", types: ["KeyPerformanceIndicator"] },
  { name: "mesto-kpi", slug: CITY_SLUG, space: "banskabystrica-kpi", types: ["KeyPerformanceIndicator"] },
  { name: "bbsk-registre", slug: REGISTER_SLUG, space: "bbsk-registre", types: ["AdministrativeArea"] },
];

const square = (x: number) => ({
  type: "Polygon",
  coordinates: [
    [
      [x, 48.7],
      [x + 0.1, 48.7],
      [x + 0.1, 48.8],
      [x, 48.7],
    ],
  ],
});

const DISTRICTS = [
  ["SK0321", "Okres Banská Bystrica", 19.1],
  ["SK0322", "Okres Brezno", 19.6],
  ["SK0325", "Okres Zvolen", 19.1],
].map(([code, name, x]) => ({
  id: `urn:ngsi-ld:AdministrativeArea:bbsk.sk:bbsk-registre:${code}`,
  type: "AdministrativeArea",
  name: { type: "LanguageProperty", languageMap: { sk: name } },
  divisionLevel: { type: "Property", value: "district" },
  location: { type: "GeoProperty", value: square(x as number) },
}));

const answer = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function show(register: () => Response, endpoints = ENDPOINTS) {
  const asked: string[] = [];
  const replies: Record<string, () => Response> = {
    [REGION_SLUG]: () => answer(region),
    [CITY_SLUG]: () => answer(city),
    [REGISTER_SLUG]: register,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      asked.push(path);
      const slug = /\/api\/endpoint\/([^/]+)\//.exec(path)?.[1];
      const reply = slug ? replies[slug] : undefined;
      if (!reply) throw new Error(`no stub for ${path}`);
      return reply();
    }),
  );
  const client = stubClient(undefined, {
    slug: REGION_SLUG,
    orgDomain: "bbsk.sk",
    space: "bbsk-kpi",
    transport: "origin",
    appName: "bbsk-ukazovatele",
    language: "sk",
    basemap: BASEMAP,
    endpoints,
  });
  const view = render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return { asked, view };
}

const last = () => drawn[drawn.length - 1];
const digits = (text: string) => text.replace(/\D/g, "");

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  drawn.length = 0;
  paints.length = 0;
  styles.length = 0;
  for (const key of Object.keys(clicks)) delete clicks[key];
});

describe("the district map", () => {
  it("draws one shape per district, coloured by the picked indicator, on the platform's basemap", async () => {
    const { asked } = show(() => answer(DISTRICTS));
    const panel = await screen.findByRole("region", { name: "Mapa okresov" });
    // The register is asked for its districts alone, in Slovak, whatever the page's language.
    const read = asked.find((path) => path.includes(REGISTER_SLUG)) ?? "";
    expect(decodeURIComponent(read)).toContain("type=AdministrativeArea");
    expect(decodeURIComponent(read)).toContain('q=divisionLevel=="district"');
    // The map is built after the panel shows, a beat later on a loaded runner.
    await waitFor(() => expect(styles).toEqual([BASEMAP]));

    fireEvent.change(within(panel).getByRole("combobox", { name: /Ukazovateľ na mape/ }), {
      target: { value: "obyvatelstvo-stav" },
    });
    await waitFor(() =>
      expect(last().features.map((feature) => [feature.id, feature.properties.value])).toEqual([
        ["okres-banska-bystrica", 106000],
        ["okres-brezno", 57517],
        ["okres-zvolen", 65001],
      ]),
    );
    // The ramp spans the picked indicator's range, from the tokens' colours.
    await waitFor(() => expect(JSON.stringify(paints[paints.length - 1])).toContain("57517"));
    expect(JSON.stringify(paints[paints.length - 1])).toContain("106000");
    const legend = panel.querySelector(".legend")!;
    expect(digits(legend.textContent ?? "")).toContain("57517");
    expect(screen.getByRole("application", { name: /Mapa okresov/ })).toBeTruthy();
  });

  it("colours each bar as the map colours its district, never in the legend's 'not measured' colour", async () => {
    show(() => answer(DISTRICTS));
    const panel = await screen.findByRole("region", { name: "Mapa okresov" });
    fireEvent.change(within(panel).getByRole("combobox", { name: /Ukazovateľ na mape/ }), {
      target: { value: "obyvatelstvo-stav" },
    });
    const chart = document.getElementById("chart-bbsk-obyvatelstvo-stav")!.closest("figure")!;
    const barOf = (name: RegExp) =>
      (within(chart).getByRole("button", { name }).closest("li")!.querySelector(".bar") as HTMLElement).style.getPropertyValue(
        "--bar",
      );
    const { low, high, point } = mapColors();
    // The map's range is 57517 (Brezno) to 106000 (Banská Bystrica): the ends of the scale, and
    // Zvolen's 65001 the same share of the way the map's fill puts it.
    await waitFor(() => expect(barOf(/Brezno/)).toBe(rampColor(57517, [57517, 106000], { low, high })));
    expect(barOf(/Brezno/)).toBe(`color-mix(in srgb, ${low}, ${high} 0%)`);
    expect(barOf(/Banská Bystrica/)).toBe(`color-mix(in srgb, ${low}, ${high} 100%)`);
    expect(barOf(/Zvolen/)).toBe(rampColor(65001, [57517, 106000], { low, high }));
    // Every chart under the map is on the scale, the unmapped ones over their own range.
    const bars = [...document.querySelectorAll<HTMLElement>(".body-bbsk .bar")];
    expect(bars.length).toBeGreaterThan(3);
    for (const bar of bars) {
      expect(bar.style.getPropertyValue("--bar")).toMatch(/^color-mix\(in srgb, /);
      expect(bar.style.getPropertyValue("--bar")).not.toContain(point);
    }
  });

  it("marks a district's bar when the district is clicked, and the district when its name is", async () => {
    show(() => answer(DISTRICTS));
    const panel = await screen.findByRole("region", { name: "Mapa okresov" });
    const key = (within(panel).getByRole("combobox") as HTMLSelectElement).value;
    const chart = document.getElementById(`chart-bbsk-${key}`)!.closest("figure")!;
    await waitFor(() => expect(clicks.areas).toBeDefined());

    clicks.areas({ features: [{ properties: { id: "okres-brezno" } }] });
    await waitFor(() =>
      expect(within(chart).getByRole("button", { name: /Brezno/ }).getAttribute("aria-pressed")).toBe("true"),
    );

    fireEvent.click(within(chart).getByRole("button", { name: /Zvolen/ }));
    await waitFor(() =>
      expect(last().features.find((feature) => feature.id === "okres-zvolen")?.properties.chosen).toBe(true),
    );
    expect(within(chart).getByRole("button", { name: /Brezno/ }).getAttribute("aria-pressed")).toBe("false");
  });

  it("has no axe violation with the map on screen", async () => {
    const { view } = show(() => answer(DISTRICTS));
    await screen.findByRole("region", { name: "Mapa okresov" });
    const result = await axe.run(view.container);
    expect(result.violations.map((violation) => violation.id)).toEqual([]);
  });
});

describe("without the district outlines", () => {
  it("draws no map where the configuration names no register endpoint, and asks it nothing", async () => {
    const { asked } = show(() => answer(DISTRICTS), ENDPOINTS.slice(0, 2));
    await screen.findAllByRole("figure");
    expect(screen.queryByRole("region", { name: "Mapa okresov" })).toBeNull();
    expect(asked.some((path) => path.includes(REGISTER_SLUG))).toBe(false);
    // The charts stay; a name opens its indicator and marks nothing, there being no map to mark on.
    expect([...document.querySelectorAll("button.bar-label")].every((bar) => bar.getAttribute("aria-pressed") === "false")).toBe(true);
    // With no map there is no scale to match: the bars keep the body's colour.
    const bars = [...document.querySelectorAll<HTMLElement>(".bar")];
    expect(bars.length).toBeGreaterThan(0);
    expect(bars.every((bar) => bar.style.getPropertyValue("--bar") === "")).toBe(true);
  });

  it("says the outlines could not be read and keeps every indicator on screen", async () => {
    show(() => answer({ title: "Refused", status: 503, detail: "register down" }, 503));
    expect(await screen.findByText(/Hranice okresov sa nepodarilo načítať/)).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Mapa okresov" })).toBeNull();
    expect(screen.getAllByRole("article").length).toBeGreaterThan(10);
  });
});
