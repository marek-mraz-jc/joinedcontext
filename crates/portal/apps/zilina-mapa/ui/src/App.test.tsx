/**
 * The Žilina map over what the public endpoint of `zilina-verejne` answers (T-3140).
 *
 * `fetch` is what is stubbed, so every case goes through the SDK's own endpoint source. MapLibre
 * needs WebGL, which jsdom does not have, so the library is a double recording what it was given.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";

const setData = vi.fn();
/** What the map was told to call on a click on its points. */
const clicks: Array<(event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void> = [];

vi.mock("maplibre-gl", () => {
  class Map {
    on(event: string, layerOrHandler: unknown, handler?: unknown) {
      if (event === "load" && typeof layerOrHandler === "function") layerOrHandler();
      if (event === "click" && typeof handler === "function") clicks.push(handler as (typeof clicks)[number]);
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    getSource = () => ({ setData });
    remove = vi.fn();
  }
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const App = (await import("./App")).default;
const { MOST, reasonOf } = await import("./App");
const { KINDS } = await import("./places");
const { answer, MONUMENTS } = await import("./fixtures/verejne");
const { LOCALES } = await import("./locales");
const s = LOCALES.sk;

const SLUG = "zhu42lbyivllciy7pluenlhxmqn5hi7c";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const CONFIG = { slug: SLUG, orgDomain: "zilina.sk", space: "zilina-verejne", transport: "origin" as const, appName: "zilina-mapa", language: "sk" };

/** The App over a `fetch` of its own, the panel's reads from the client's stub. */
function mount(config: Record<string, unknown> = {}) {
  const rows = ["PointOfInterest", "GtfsStop", "AirQualityObserved"].flatMap((type) => answer(type)) as Row[];
  const client = stubClient({ entities: rows }, { ...CONFIG, portal: "https://portal.zilina.sk/projects/zilina", ...config });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

function show(refuse?: string, withEndpoint = true, basemap?: string) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      const type = new URL(path, "http://portal.test").searchParams.get("type");
      if (type === refuse) {
        return new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "the policy does not grant this type" }), {
          status: 403,
          headers: { "content-type": "application/problem+json" },
        });
      }
      return json(answer(type));
    }),
  );
  return mount({ slug: withEndpoint ? SLUG : "", space: withEndpoint ? "zilina-verejne" : "elsewhere", ...(basemap ? { basemap } : {}) });
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const results = () => screen.getByRole("region", { name: /miest|place/ });
const names = () => within(results()).queryAllByRole("button").map((button) => button.querySelector(".name")?.textContent);

describe("the Žilina map", () => {
  it("lists the monuments, the stations busiest first and the air station, and draws those with a position", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Stanica SK0020A, mestské pozadie"));
    expect(names()).toHaveLength(11);
    expect(names().slice(5, 7)).toEqual(["Žilina", "Brodno"]);
    expect(within(results()).getByText("166 odchodov dnes")).toBeInTheDocument();
    // The column on a square has no address: listed and said so, not drawn.
    expect(within(results()).getByText(new RegExp(s.notOnMap))).toBeInTheDocument();
    await waitFor(() => {
      const data = setData.mock.calls.at(-1)?.[0] as { features: unknown[] } | undefined;
      expect(data?.features).toHaveLength(10);
    });
  });

  it("shows one kind at a time and searches without diacritics", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Žilina"));
    await userEvent.click(screen.getByRole("radio", { name: new RegExp(`^${s.kind.monument}`) }));
    expect(names()).not.toContain("Žilina");
    expect(names()).toHaveLength(5);
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "kastiel");
    expect(names()).toEqual(["Kaštieľ", "Kaštieľ Bytčica"]);
    await userEvent.clear(screen.getByRole("searchbox", { name: s.search }));
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "nic take");
    expect(within(results()).getByText(s.noResults)).toBeInTheDocument();
  });

  // SDK-40, AP-140: a public App opens a place in the panel, which links to the Portal and edits nothing.
  it("opens a place from the list in the entity panel, with a Portal link and no Edit", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Stanica SK0020A, mestské pozadie"));
    const item = within(results()).getByRole("button", { name: /Stanica SK0020A/ });
    await userEvent.click(item);
    const panel = await screen.findByRole("dialog");
    expect(item).toHaveAttribute("aria-pressed", "true");
    await waitFor(() => expect(within(panel).getByRole("link", { name: "Otvoriť v Portáli" })).toBeInTheDocument());
    expect(within(panel).queryByRole("button", { name: "Upraviť" })).toBeNull();
    await userEvent.click(within(panel).getByRole("button", { name: "Zavrieť" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(item).toHaveAttribute("aria-pressed", "false");
  });

  it("opens every listed place, and Escape closes the panel", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Žilina"));
    for (const button of within(results()).getAllByRole("button")) {
      await userEvent.click(button);
      expect(await screen.findByRole("dialog")).toBeInTheDocument();
    }
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens a place a person clicks on the map; a click beside every point opens nothing", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Trojičný stĺp"));
    const click = clicks.at(-1);
    expect(click).toBeDefined();
    act(() => click?.({ features: [] }));
    expect(screen.queryByRole("dialog")).toBeNull();
    act(() => click?.({ features: [{ properties: { id: MONUMENTS[0].id } }] }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    expect(within(results()).getByRole("button", { name: /Trojičný stĺp/ })).toHaveAttribute("aria-pressed", "true");
    // A place no longer listed opens nothing new.
    act(() => click?.({ features: [{ properties: { id: "urn:ngsi-ld:PointOfInterest:gone" } }] }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("link", { name: "Otvoriť v Portáli" }));
  });

  it("switches kinds while they load, and counts each once loaded", async () => {
    let release: () => void = () => {};
    const held = new Promise<void>((done) => (release = done));
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => {
        await held;
        return json(answer(new URL(path, "http://portal.test").searchParams.get("type")));
      }),
    );
    mount();
    expect(await screen.findByText(s.loading)).toBeInTheDocument();
    for (const kind of KINDS) await userEvent.click(screen.getByRole("radio", { name: s.kind[kind] }));
    await userEvent.click(screen.getByRole("radio", { name: s.all }));
    release();
    await waitFor(() => expect(screen.getByRole("radio", { name: `${s.kind.monument} (5)` })).toBeInTheDocument());
    for (const kind of KINDS) await userEvent.click(screen.getByRole("radio", { name: new RegExp(`^${s.kind[kind]} \\(`) }));
    expect(names()).toEqual(["Stanica SK0020A, mestské pozadie"]);
  });

  it("shows the first places of a kind that has more than the map holds, and says so", async () => {
    const many = Array.from({ length: 200 }, (_, i) => ({ ...MONUMENTS[0], id: `urn:ngsi-ld:PointOfInterest:zilina.sk:zilina-verejne:${i}` }));
    let release: () => void = () => {};
    const held = new Promise<void>((done) => (release = done));
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => {
        await held;
        const type = new URL(path, "http://portal.test").searchParams.get("type");
        return json(type === "PointOfInterest" ? many : answer(type));
      }),
    );
    mount();
    // Narrowed first, so the list holds one station, not a thousand monuments.
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "brodno");
    release();
    expect(await screen.findByText(s.truncated(s.kind.monument, MOST))).toBeInTheDocument();
    expect(names()).toEqual(["Brodno"]);
  });

  it("says a failure that is no answer of the endpoint in its own words", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("the network is down");
      }),
    );
    mount();
    const alerts = await screen.findAllByRole("alert");
    expect(alerts.map((alert) => alert.textContent).join(" ")).toContain("the network is down");
  });

  it("lists a place the registers say little about, in English when the reader reads English", async () => {
    const id = (type: string) => `urn:ngsi-ld:${type}:zilina.sk:zilina-verejne:sparse`;
    const sparse: Record<string, unknown[]> = {
      PointOfInterest: [{ id: id("PointOfInterest"), type: "PointOfInterest" }],
      GtfsStop: [{ id: id("GtfsStop"), type: "GtfsStop", name: { type: "Property", value: "Lietavská Lúčka" } }],
      AirQualityObserved: [
        { id: id("AirQualityObserved"), type: "AirQualityObserved", name: { type: "Property", value: "Bez PM10" } },
        { id: `${id("AirQualityObserved")}2`, type: "AirQualityObserved", name: { type: "Property", value: "PM10 v mg" }, pm10: { type: "Property", value: 0.0321, unitCode: "GP" } },
      ],
    };
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => json(sparse[new URL(path, "http://portal.test").searchParams.get("type") ?? ""] ?? [])),
    );
    mount({ language: undefined });
    await waitFor(() => expect(names()).toContain("Lietavská Lúčka"));
    expect(within(results()).getByText(`${s.kind.monument} · ${s.notOnMap}`)).toBeInTheDocument();
    expect(within(results()).getByText(`${s.kind.station} · ${s.notOnMap}`)).toBeInTheDocument();
    expect(within(results()).getByText(`${s.kind.air} · ${s.notOnMap}`)).toBeInTheDocument();
    expect(within(results()).getByText(/PM10 0,03 mg\/m³/)).toBeInTheDocument();
    expect(names()).toContain(s.unnamed);
    // Each opens all the same: the panel reads what the registers hold.
    for (const button of within(results()).getAllByRole("button")) {
      await userEvent.click(button);
      expect(await screen.findByRole("dialog")).toBeInTheDocument();
    }
  });

  it("speaks English to an English reader, the places' names too, every control in English", async () => {
    let release: () => void = () => {};
    const held = new Promise<void>((done) => (release = done));
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => {
        await held;
        return json(answer(new URL(path, "http://portal.test").searchParams.get("type")));
      }),
    );
    mount({ language: "en" });
    const en = LOCALES.en;
    expect(screen.getByRole("heading", { level: 1, name: en.title })).toBeInTheDocument();
    for (const kind of KINDS) await userEvent.click(screen.getByRole("radio", { name: en.kind[kind] }));
    release();
    await waitFor(() => expect(screen.getByRole("radio", { name: `${en.kind.monument} (5)` })).toBeInTheDocument());
    for (const kind of KINDS) await userEvent.click(screen.getByRole("radio", { name: new RegExp(`^${en.kind[kind]} \\(`) }));
    await userEvent.click(screen.getByRole("radio", { name: en.all }));
    expect(within(results()).getByRole("button", { name: /Station SK0020A, urban background/ })).toBeInTheDocument();
    for (const button of within(results()).getAllByRole("button")) {
      await userEvent.click(button);
      expect(await screen.findByRole("dialog")).toBeInTheDocument();
    }
    const panel = screen.getByRole("dialog");
    await userEvent.click(await within(panel).findByRole("link", { name: "Open in the Portal" }));
    await userEvent.click(within(panel).getByRole("button", { name: "Close" }));
    await userEvent.type(screen.getByRole("searchbox", { name: en.search }), "brodno");
    expect(names()).toEqual(["Brodno"]);
  });

  it("says a failure that is not even an error as it came", () => {
    expect(reasonOf("the proxy hung up")).toBe("the proxy hung up");
    expect(reasonOf(new Error("refused"))).toBe("refused");
  });

  it("names a missing base map, and draws the configured one without a notice", () => {
    const first = show();
    expect(screen.getByText(s.noBasemap)).toBeInTheDocument();
    first.unmount();
    show(undefined, true, "https://tiles.example.org/style.json");
    expect(screen.queryByText(s.noBasemap)).toBeNull();
  });

  it("says why a kind the endpoint refuses is missing and keeps the others", async () => {
    show("GtfsStop");
    expect(await screen.findByRole("alert")).toHaveTextContent(s.refused(s.kind.station, "the policy does not grant this type"));
    await waitFor(() => expect(names()).toContain("Trojičný stĺp"));
    expect(names()).not.toContain("Brodno");
  });

  it("says it has nothing to read when the app has no endpoint of the public space", () => {
    show(undefined, false);
    expect(screen.getByRole("status")).toHaveTextContent(s.noEndpoint);
  });

  it("names its sources and has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(names()).toContain("Žilina"));
    expect(screen.getByText(s.attribution)).toBeInTheDocument();
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
