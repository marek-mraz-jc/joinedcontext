/**
 * The screen, over what the endpoint of `banskabystrica-verejne` answers (T-2435, T-2949).
 *
 * `fetch` is what is stubbed and nothing below it, so every case goes through the SDK's own
 * endpoint source: the same URL building, the same NGSI-LD parsing and the same refusal handling
 * the published bundle uses. MapLibre needs WebGL, which jsdom does not have, so the library is
 * replaced by a double that records what the app asked it to draw.
 */
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider, projectRow, toRichRow } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";

const setData = vi.fn();
const addSource = vi.fn();
const addLayer = vi.fn();
const remove = vi.fn();
/** The style each map was built with: the platform's URL, or the SDK's plain background. */
const styles: unknown[] = [];
/** What MapLibre would call on a click of a drawn station. */
const map = vi.hoisted(() => ({ click: null as ((event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void) | null }));

vi.mock("maplibre-gl", () => {
  class Map {
    constructor(public options: { style?: unknown }) {
      styles.push(options.style);
    }
    on(event: string, layerOrHandler: unknown, handler?: (event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void) {
      // The app adds its source and layer on `load`; firing it at once is what the browser does
      // before anybody looks at the page.
      if (event === "load" && typeof layerOrHandler === "function") layerOrHandler();
      if (event === "click" && handler) map.click = handler;
    }
    addSource = addSource;
    addLayer = addLayer;
    getSource = () => ({ setData });
    remove = remove;
  }
  // Named, not default: maplibre-gl 6 has no default export.
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const App = (await import("./App")).default;
const { answer, history, sk0263a } = await import("./fixtures/verejne");
const { LOCALES } = await import("./locales");

const SLUG = "mluyob4nz52lok3ssk7pgn5vwt";
const NOW = new Date("2026-09-20T09:00:00Z");

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const problem = (status: number, detail: string) =>
  new Response(JSON.stringify({ title: "Refused", status, detail }), {
    status,
    headers: { "content-type": "application/problem+json" },
  });

let calls: Array<{ path: string; init?: RequestInit }>;

/** Answers the entity query and the temporal read; anything else is a test that asked wrongly. */
function serving(options: { entities?: () => Response; temporal?: () => Response } = {}) {
  return vi.fn(async (path: string, init?: RequestInit) => {
    calls.push({ path, init });
    if (path.includes("/temporal/entities/")) {
      return options.temporal ? options.temporal() : json(history(`${path}`, NOW));
    }
    if (path.includes("/entities")) {
      return options.entities ? options.entities() : json(answer(NOW));
    }
    throw new Error(`no stub for ${path}`);
  });
}

function show(
  options: Parameters<typeof serving>[0] = {},
  language = "sk",
  basemap?: string,
  over: { space?: string; endpoints?: { name: string; slug: string; space: string; types: string[] }[] } = {},
) {
  vi.stubGlobal("fetch", serving(options));
  // The cards read through `fetch`, the entity panel through the client: the same stations on both.
  const client = stubClient({ entities: answer(NOW).map((entity) => projectRow(toRichRow(entity as Record<string, unknown>, "sk"), "sk")) }, {
    portal: "https://portal.banskabystrica.sk/projects/banskabystrica",
    slug: SLUG,
    orgDomain: "banskabystrica.sk",
    space: over.space ?? "banskabystrica-verejne",
    ...(over.endpoints ? { endpoints: over.endpoints } : {}),
    transport: "origin",
    appName: "banskabystrica-ovzdusie",
    language,
    basemap,
  });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

beforeEach(() => {
  calls = [];
  styles.length = 0;
  vi.useFakeTimers({ shouldAdvanceTime: true });
  vi.setSystemTime(NOW);
  vi.unstubAllGlobals();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const stations = () => screen.getByRole("region", { name: LOCALES.sk.stationsLabel });

describe("the stations of the city", () => {
  // T-2949: the city's one real station, as the EEA pipelines write it, is on the screen in
  // Banská Bystrica under the name it publishes, with a fresh reading and not an old one.
  it("shows station SK0263A in Banská Bystrica under its published name with its latest reading", async () => {
    show({ entities: () => json([sk0263a(NOW, 40)]) });
    const card = await waitFor(() =>
      within(stations()).getByRole("heading", { name: "Stanica SK0263A, mestské pozadie" }).closest("article"),
    );
    expect(within(card as HTMLElement).getByText(/21,3|21\.3/)).toBeInTheDocument();
    expect(within(card as HTMLElement).queryByText(new RegExp(LOCALES.sk.band.stale))).toBeNull();
    // The newest source is the one to read, once it holds the station (T-2994).
    await waitFor(() => {
      const [, source] = (addSource.mock.calls.at(-1) ?? []) as [string, { data: { features: Array<{ geometry: { coordinates: number[] } }> } }];
      expect(source?.data.features.map((feature) => feature.geometry.coordinates)).toEqual([[19.115268, 48.733256]]);
    });
    // The read went to the configured endpoint of the city's public space.
    expect(calls.some((call) => call.path.includes(`/api/endpoint/${SLUG}/ngsi-ld/v1/entities`))).toBe(true);
    expect(screen.getByText(LOCALES.sk.source)).toBeInTheDocument();
    // Its card opens it in the entity panel by its published name.
    fireEvent.click(within(card as HTMLElement).getByRole("button", { name: "Podrobnosti: Stanica SK0263A, mestské pozadie" }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("draws one point per station that publishes a place, and lists every station", async () => {
    show();
    await waitFor(() => expect(addSource).toHaveBeenCalled());

    const [, source] = addSource.mock.calls[0] as [string, { data: { features: unknown[] } }];
    // Four stations answered; the fourth publishes no location, so it is on the list and not on
    // the map — leaving it off the list would hide a station that exists.
    expect(source.data.features).toHaveLength(3);
    expect(within(stations()).getAllByRole("article")).toHaveLength(4);
    expect(within(stations()).getByRole("heading", { name: "Stanica 4" })).toBeInTheDocument();
    expect(within(stations()).getByText(LOCALES.sk.noLocation)).toBeInTheDocument();
  });

  it("marks a station that stopped reporting as old, not as clean air", async () => {
    show();
    const card = await waitFor(() =>
      within(stations()).getByRole("heading", { name: "Stanica 3" }).closest("article"),
    );
    // Its last number is 9.1 µg/m³, which would be the cleanest reading on the screen.
    expect(within(card as HTMLElement).getByText(/9,1|9\.1/)).toBeInTheDocument();
    expect(within(card as HTMLElement).getByText(new RegExp(LOCALES.sk.band.stale))).toBeInTheDocument();
    expect(within(card as HTMLElement).queryByText(new RegExp(LOCALES.sk.band.low))).toBeNull();
  });

  it("says a station is over the limit when it is", async () => {
    show();
    const card = await waitFor(() =>
      within(stations()).getByRole("heading", { name: "Stanica 2" }).closest("article"),
    );
    expect(within(card as HTMLElement).getByText(new RegExp(LOCALES.sk.band.high))).toBeInTheDocument();
  });

  it("says there is no station rather than drawing an empty map", async () => {
    show({ entities: () => json([]) });
    await waitFor(() => expect(screen.getByText(new RegExp(LOCALES.sk.empty))).toBeInTheDocument());
    expect(screen.queryByRole("region", { name: LOCALES.sk.stationsLabel })).toBeNull();
    expect(addSource).not.toHaveBeenCalled();
  });

  it("shows a refusal from the endpoint as itself, in the endpoint's own words", async () => {
    show({ entities: () => problem(503, "Broker je nedostupný.") });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Broker je nedostupný.");
  });
});

describe("the picked station", () => {
  it("opens on the freshest station and reads one day of it through retrieveTemporal", async () => {
    show();
    await waitFor(() => expect(screen.getByRole("region", { name: /Stanica 4/ })).toBeInTheDocument());
    await waitFor(() =>
      expect(calls.some((call) => call.path.includes("/temporal/entities/"))).toBe(true),
    );
    const temporal = calls.find((call) => call.path.includes("/temporal/entities/"))!;
    expect(temporal.path).toContain("attrs=pm10");
    expect(temporal.path).toContain("timerel=");
  });

  // T-2991: the day is a coloured line under the App's own heading, the values in Slovak with
  // one decimal, and the station's URN is nowhere on the panel.
  it("draws the day as a chart a person reads", async () => {
    show();
    const heading = `${LOCALES.sk.historyOf} ${LOCALES.sk.pm10}`;
    const chart = await screen.findByRole("img", { name: heading });
    expect(chart.querySelector("polyline")?.getAttribute("stroke")).not.toBe("currentColor");
    expect(within(chart).getByText(LOCALES.sk.unit)).toBeInTheDocument();
    const panel = screen.getByRole("region", { name: heading });
    expect(panel.textContent).not.toContain("urn:ngsi-ld");
    const values = [...within(panel).getByRole("table").querySelectorAll("tbody td:nth-child(2)")];
    expect(values.length).toBeGreaterThan(0);
    for (const cell of values) expect(cell.textContent).toMatch(/^\d+(,\d)?$/);
  });

  // T-2972: the endpoint answers an addressed temporal read of an entity with no instances (or
  // none its grants reach) with 404; the panel says nothing was recorded, never the raw error.
  it("says no value was recorded when the station has no history yet", async () => {
    show({
      entities: () => json([sk0263a(NOW, 40)]),
      temporal: () => problem(404, "Resource Not Found"),
    });
    const panel = await screen.findByRole("region", { name: /Stanica SK0263A/ });
    expect(await within(panel).findByText(LOCALES.sk.history.empty)).toBeInTheDocument();
    expect(within(panel).queryByText(/Resource Not Found/)).toBeNull();
  });

  it("changes to the station a person picks", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    show();
    const card = await waitFor(() =>
      within(stations()).getByRole("heading", { name: "Stanica 2" }).closest("article"),
    );
    await user.click(within(card as HTMLElement).getByRole("button", { name: LOCALES.sk.pick }));
    expect(screen.getByRole("region", { name: /Stanica 2/ })).toBeInTheDocument();
  });
});

describe("what the screen says about itself", () => {
  it("is Slovak by default and English when the configuration asks", async () => {
    const { unmount } = show();
    expect(await screen.findByRole("heading", { name: LOCALES.sk.title, level: 1 })).toBeInTheDocument();
    unmount();

    show({}, "en");
    expect(await screen.findByRole("heading", { name: LOCALES.en.title, level: 1 })).toBeInTheDocument();
  });

  it("never sends a credential: the endpoint is public and the bundle holds no token (AP-28)", async () => {
    show();
    await waitFor(() => expect(calls.length).toBeGreaterThan(0));
    for (const call of calls) {
      const headers = (call.init?.headers ?? {}) as Record<string, string>;
      expect(Object.keys(headers).map((name) => name.toLowerCase())).not.toContain("authorization");
      expect(call.init?.credentials).toBe("same-origin");
    }
  });

  it("has no axe violation with the stations on screen", async () => {
    const { container } = show();
    await waitFor(() => expect(screen.getByRole("region", { name: LOCALES.sk.stationsLabel })).toBeInTheDocument());
    const results = await axe.run(container, {
      rules: { "color-contrast": { enabled: false }, region: { enabled: false } },
    });
    expect(results.violations.map((violation) => violation.id)).toEqual([]);
  });

  it("draws the stations over the platform's basemap and names no tile host of its own (AP-67)", async () => {
    const basemap = "https://portal.example/api/v1/projects/bbsk/basemap/default/style.json";
    show({}, "sk", basemap);
    await waitFor(() => expect(styles).toEqual([basemap]));
    expect(screen.queryByText(LOCALES.sk.noBasemap)).toBeNull();
  });

  it("says so under the map when the platform configures no basemap", async () => {
    show({}, "en");
    await waitFor(() => expect(styles).toHaveLength(1));
    const [style] = styles as Array<{ sources?: Record<string, unknown> }>;
    // The SDK's plain background: no source, so no request leaves for a tile host.
    expect(style.sources).toEqual({});
    expect(screen.getByText(LOCALES.en.noBasemap)).toBeVisible();
  });
});

describe("the entity panel (SDK-40)", () => {
  it.each([
    ["sk", "Podrobnosti", "Zavrieť", "Otvoriť v Portáli", "Upraviť"],
    ["en", "Details", "Close", "Open in the Portal", "Edit"],
  ] as const)("in %s, each station opens from its card, linked to the Portal, with no Edit", async (language, details, close, portal, edit) => {
    show({}, language);
    const buttons = await screen.findAllByRole("button", { name: new RegExp(`^${details}: `) });
    for (const button of buttons) {
      fireEvent.click(button);
      const panel = await screen.findByRole("dialog");
      const link = await within(panel).findByRole("link", { name: portal });
      link.addEventListener("click", (event) => event.preventDefault());
      fireEvent.click(link);
      expect(within(panel).queryByRole("button", { name: edit })).toBeNull();
      fireEvent.click(within(panel).getByRole("button", { name: close }));
    }
    // Each card's pick button picks its station for the history; the picked one stays picked.
    for (const pick of screen.getAllByRole("button", { pressed: false })) fireEvent.click(pick);
    fireEvent.click(screen.getAllByRole("button", { pressed: true })[0]);
    expect(screen.queryByRole("dialog")).toBeNull();
    // The history's own controls: each range, and the copy of the day as CSV.
    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const ranges = await screen.findAllByRole("radio");
    // The last range is "between": its two days are typed, then a fixed range is chosen again.
    fireEvent.click(ranges[ranges.length - 1]);
    for (const label of language === "sk" ? ["Od", "Do"] : ["From", "To"]) {
      const day = await screen.findByLabelText(label);
      fireEvent.change(day, { target: { value: (day as HTMLInputElement).type === "date" ? "2026-09-19" : "2026-09-19T08:00" } });
    }
    for (const range of ranges.slice(0, -1).reverse()) fireEvent.click(range);
    fireEvent.click(await screen.findByRole("button", { name: language === "sk" ? "Kopírovať ako CSV" : "Copy as CSV" }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
  });

  it("picks a station clicked on the map and opens it, and ignores a click on nothing", async () => {
    show();
    await screen.findAllByRole("button", { name: /^Podrobnosti: / });
    await waitFor(() => expect(map.click).not.toBeNull());
    act(() => map.click?.({ features: [] }));
    expect(screen.queryByRole("dialog")).toBeNull();
    act(() => map.click?.({ features: [{ properties: { id: String(answer(NOW)[0].id) } }] }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });
});

describe("the edges of reading", () => {
  it("reads through the endpoint the Portal lists for the space, and says so when there is none", async () => {
    show({}, "sk", undefined, { space: "elsewhere", endpoints: [{ name: "verejne", slug: SLUG, space: "banskabystrica-verejne", types: [] }] });
    expect(await screen.findAllByRole("button", { name: /^Podrobnosti: / })).not.toHaveLength(0);
    cleanup();
    show({}, "sk", undefined, { space: "elsewhere" });
    expect(screen.getByText(LOCALES.sk.noEndpoint)).toBeInTheDocument();
  });

  it("says a failure that is no endpoint answer in its own words, and drops answers after it is gone", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => {
      throw "offline";
    }));
    const client = stubClient(undefined, { slug: SLUG, orgDomain: "banskabystrica.sk", space: "banskabystrica-verejne", transport: "origin", appName: "banskabystrica-ovzdusie", language: "sk" });
    const first = render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("offline");
    first.unmount();
    for (const fails of [false, true]) {
      let reply: () => void = () => undefined;
      vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>((resolve, reject) => (reply = () => (fails ? reject(new TypeError("late")) : resolve(json(answer(NOW))))))));
      const view = render(
        <JcProvider client={client}>
          <App />
        </JcProvider>,
      );
      view.unmount();
      const errors = vi.spyOn(console, "error");
      reply();
      await new Promise((settle) => setTimeout(settle, 0));
      expect(errors).not.toHaveBeenCalled();
    }
  });

  it("says a station has no reading time when it publishes none", async () => {
    const bare = { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:banskabystrica-verejne:bare", type: "AirQualityObserved", name: { type: "Property", value: "Bez času" }, pm10: { type: "Property", value: 5 } };
    show({ entities: () => json([bare]) });
    const card = (await screen.findByRole("heading", { name: "Bez času" })).closest("article") as HTMLElement;
    expect(within(card).getAllByText(LOCALES.sk.noReading).length).toBeGreaterThan(0);
    fireEvent.click(within(card).getByRole("button", { name: "Podrobnosti: Bez času" }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });
});

