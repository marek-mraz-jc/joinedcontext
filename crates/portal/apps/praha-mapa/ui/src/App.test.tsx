/**
 * The Prague map over what the public endpoint of `praha-mesto` answers (T-2786).
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
const addSource = vi.fn();
/** What the map was told to call on a click on its points. */
const clicks: Array<(event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void> = [];
/** What the map calls after a pan or zoom, and the part it says is in view. */
const moves: Array<() => void> = [];
let bounds: [number, number, number, number] = [14.3, 50.0, 14.5, 50.1];

vi.mock("maplibre-gl", () => {
  class Map {
    on(event: string, layerOrHandler: unknown, handler?: unknown) {
      if (event === "load" && typeof layerOrHandler === "function") layerOrHandler();
      if (event === "click" && typeof handler === "function") clicks.push(handler as (typeof clicks)[number]);
      if (event === "moveend" && typeof layerOrHandler === "function") moves.push(layerOrHandler as () => void);
    }
    getBounds = () => ({ getWest: () => bounds[0], getSouth: () => bounds[1], getEast: () => bounds[2], getNorth: () => bounds[3] });
    addSource = addSource;
    addLayer = vi.fn();
    getSource = () => ({ setData });
    remove = vi.fn();
  }
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const App = (await import("./App")).default;
const { areaOf, MOST, reasonOf } = await import("./App");
const { KINDS } = await import("./places");
const { answer } = await import("./fixtures/praha");
const { LOCALES } = await import("./locales");
const s = LOCALES.cs;

const SLUG = "qb3wx7nc5ztk2pmyr6hjd4vae2";
/** The id of the fixture's point of interest of that name. */
const POI_ID = (name: string) => {
  const row = answer("PointOfInterest").find((candidate) => JSON.stringify(candidate).includes(name)) as { id: string };
  return row.id;
};

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function show(refuse?: string, withEndpoint = true, basemap?: string) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      const url = new URL(path, "http://portal.test");
      const type = url.searchParams.get("type");
      if (type === refuse) {
        return new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "the policy does not grant this type" }), {
          status: 403,
          headers: { "content-type": "application/problem+json" },
        });
      }
      return json(answer(type));
    }),
  );
  // The list reads through `fetch` above; the entity panel reads one place through the client (SDK-40).
  const rows = [...answer("PointOfInterest"), ...answer("WasteContainerIsle")] as Row[];
  const client = stubClient({ entities: rows }, {
    portal: "https://portal.praha.eu/projects/praha",
    slug: withEndpoint ? SLUG : "",
    orgDomain: "praha.eu",
    space: withEndpoint ? "praha-mesto" : "elsewhere",
    transport: "origin",
    appName: "praha-mapa",
    language: "cs",
    ...(basemap ? { basemap } : {}),
  });
  return render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const results = () => screen.getByRole("region", { name: /míst/ });
const names = () => within(results()).queryAllByRole("button").map((button) => button.querySelector(".name")?.textContent);

describe("the Prague map", () => {
  it("lists schools, culture, toilets and ticket points, the isles one checkbox away", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Národní divadlo"));
    expect(names()).toEqual(["Galerie se skriptem", "Národní divadlo", "Předprodej jízdenek Muzeum", "Veřejné WC Anděl", "Základní škola Vodičkova"]);
    // A point of interest of a category the map does not show is not shown under a wrong one.
    expect(names()).not.toContain("Místo bez kategorie");
    await userEvent.click(screen.getByRole("checkbox", { name: new RegExp(`^${s.kind.waste}`) }));
    expect(names()).toContain("Vinohradská 12");
    await waitFor(() => {
      const data = setData.mock.calls.at(-1)?.[0] as { features: unknown[] } | undefined;
      expect(data?.features).toHaveLength(6);
    });
  });

  it("counts each layer and narrows by layer and by search, without diacritics", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Národní divadlo"));
    expect(screen.getByRole("checkbox", { name: `${s.kind.culture} (2)` })).toBeChecked();
    await userEvent.click(screen.getByRole("checkbox", { name: `${s.kind.culture} (2)` }));
    expect(names()).not.toContain("Národní divadlo");
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "zakladni skola");
    expect(names()).toEqual(["Základní škola Vodičkova"]);
    await userEvent.clear(screen.getByRole("searchbox", { name: s.search }));
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "nic takoveho");
    expect(within(results()).getByText(s.noResults)).toBeInTheDocument();
  });

  it("opens a place from the list in the entity panel, with a Portal link and no Edit", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Veřejné WC Anděl"));
    const item = within(results()).getByRole("button", { name: /WC Anděl/ });
    await userEvent.click(item);
    const panel = await screen.findByRole("dialog");
    expect(item).toHaveAttribute("aria-pressed", "true");
    await waitFor(() => expect(within(panel).getByRole("link", { name: "Otevřít v Portálu" })).toBeInTheDocument());
    expect(within(panel).queryByRole("button", { name: "Upravit" })).toBeNull();
    await userEvent.click(within(panel).getByRole("button", { name: "Zavřít" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(item).toHaveAttribute("aria-pressed", "false");
  });

  it("opens every listed place, and Escape closes the panel", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Národní divadlo"));
    await userEvent.click(screen.getByRole("checkbox", { name: new RegExp(`^${s.kind.waste}`) }));
    for (const button of within(results()).getAllByRole("button")) {
      await userEvent.click(button);
      expect(await screen.findByRole("dialog")).toBeInTheDocument();
    }
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens a place a person clicks on the map; a click beside every point opens nothing", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Národní divadlo"));
    const click = clicks.at(-1);
    expect(click).toBeDefined();
    act(() => click?.({ features: [] }));
    expect(screen.queryByRole("dialog")).toBeNull();
    const theatre = POI_ID("Národní divadlo");
    act(() => click?.({ features: [{ properties: { id: theatre } }] }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    expect(within(results()).getByRole("button", { name: /Národní divadlo/ })).toHaveAttribute("aria-pressed", "true");
    // A place no longer listed opens nothing.
    act(() => click?.({ features: [{ properties: { id: "urn:ngsi-ld:PointOfInterest:gone" } }] }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("link", { name: "Otevřít v Portálu" }));
  });

  it("every layer can be switched off while it loads, and each is switched off once loaded", async () => {
    let release: () => void = () => {};
    const held = new Promise<void>((done) => (release = done));
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => {
        await held;
        return json(answer(new URL(path, "http://portal.test").searchParams.get("type")));
      }),
    );
    const client = stubClient(undefined, { slug: SLUG, orgDomain: "praha.eu", space: "praha-mesto", transport: "origin", appName: "praha-mapa", language: "cs" });
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    expect(await screen.findByText(s.loading)).toBeInTheDocument();
    for (const kind of KINDS) {
      await userEvent.click(screen.getByRole("checkbox", { name: s.kind[kind] }));
    }
    release();
    await waitFor(() => expect(screen.getByRole("checkbox", { name: `${s.kind.school} (1)` })).toBeInTheDocument());
    for (const name of [`${s.kind.school} (1)`, `${s.kind.publicToilet} (1)`, `${s.kind.ticketSale} (1)`]) {
      await userEvent.click(screen.getByRole("checkbox", { name }));
    }
  });

  it("shows the first places of a type that has more than the map holds, and says so", async () => {
    const many = Array.from({ length: 200 }, (_, i) => ({ ...(answer("WasteContainerIsle")[0] as object), id: `urn:ngsi-ld:WasteContainerIsle:praha.eu:praha-mesto:${i}` }));
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => {
        const type = new URL(path, "http://portal.test").searchParams.get("type");
        return json(type === "WasteContainerIsle" ? many : answer(type));
      }),
    );
    const client = stubClient(undefined, { slug: SLUG, orgDomain: "praha.eu", space: "praha-mesto", transport: "origin", appName: "praha-mapa", language: "cs" });
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    expect(await screen.findByText(s.truncated(s.type.WasteContainerIsle, MOST))).toBeInTheDocument();
    // Narrowed first, so switching the isles on lists one place, not four thousand.
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "nic takoveho");
    await userEvent.click(screen.getByRole("checkbox", { name: `${s.kind.waste} (${MOST})` }));
    expect(within(results()).getByText(s.noResults)).toBeInTheDocument();
  });

  it("past the cap reads the isles inside the map's view, and again after a pan, keeping the newest view", async () => {
    const isle = (i: number) => ({ ...(answer("WasteContainerIsle")[0] as object), id: `urn:ngsi-ld:WasteContainerIsle:praha.eu:praha-mesto:${i}` });
    const many = Array.from({ length: 200 }, (_, i) => isle(i));
    bounds = [14.3, 50.0, 14.5, 50.1];
    moves.length = 0;
    // The first view answers late, after the pan's: its places must not come back.
    let lateFirst: (() => void) | null = null;
    const areas: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => {
        const params = new URL(path, "http://portal.test").searchParams;
        const type = params.get("type");
        if (type !== "WasteContainerIsle") return json(answer(type));
        if (params.get("georel") === null) return json(many);
        expect(params.get("georel")).toBe("within");
        expect(params.get("geometry")).toBe("Polygon");
        expect(params.get("geoproperty")).toBe("location");
        const coordinates = params.get("coordinates") ?? "";
        areas.push(coordinates);
        if (coordinates.includes("14.3,")) {
          await new Promise<void>((resolve) => (lateFirst = resolve));
          return json([isle(9001), isle(9002), isle(9003), isle(9004), isle(9005)]);
        }
        return json([isle(9100), isle(9101)]);
      }),
    );
    const client = stubClient(undefined, { slug: SLUG, orgDomain: "praha.eu", space: "praha-mesto", transport: "origin", appName: "praha-mapa", language: "cs" });
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    await waitFor(() => expect(areas).toHaveLength(1));
    expect(JSON.parse(areas[0])).toEqual([[[14.3, 50], [14.5, 50], [14.5, 50.1], [14.3, 50.1], [14.3, 50]]]);
    expect(screen.getByText(s.truncated(s.type.WasteContainerIsle, MOST))).toBeInTheDocument();

    bounds = [14.6, 50.0, 14.8, 50.1];
    act(() => moves.forEach((move) => move()));
    expect(await screen.findByText(s.inView(s.type.WasteContainerIsle, 2))).toBeInTheDocument();
    await act(async () => lateFirst?.());
    expect(screen.getByText(s.inView(s.type.WasteContainerIsle, 2))).toHaveAttribute("role", "status");
    expect(screen.getByRole("checkbox", { name: `${s.kind.waste} (2)` })).toBeInTheDocument();
  });

  it("says when the view itself holds more than the map draws", async () => {
    const many = Array.from({ length: 200 }, (_, i) => ({ ...(answer("WasteContainerIsle")[0] as object), id: `urn:ngsi-ld:WasteContainerIsle:praha.eu:praha-mesto:${i}` }));
    moves.length = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => {
        const type = new URL(path, "http://portal.test").searchParams.get("type");
        return json(type === "WasteContainerIsle" ? many : answer(type));
      }),
    );
    const client = stubClient(undefined, { slug: SLUG, orgDomain: "praha.eu", space: "praha-mesto", transport: "origin", appName: "praha-mapa", language: "cs" });
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    expect(await screen.findByText(s.inViewTruncated(s.type.WasteContainerIsle, MOST))).toBeInTheDocument();
  });

  it("clamps a view past the globe's edge to it", () => {
    expect(JSON.parse(areaOf([-200, -95, 200, 95])?.coordinates ?? "")).toEqual([[[-180, -90], [180, -90], [180, 90], [-180, 90], [-180, -90]]]);
  });

  it("says a failure that is no answer of the endpoint in its own words", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("the network is down");
      }),
    );
    const client = stubClient(undefined, { slug: SLUG, orgDomain: "praha.eu", space: "praha-mesto", transport: "origin", appName: "praha-mapa", language: "cs" });
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    const alerts = await screen.findAllByRole("alert");
    expect(alerts.map((alert) => alert.textContent).join(" ")).toContain("the network is down");
  });

  it("says a failure that is not even an error as it came", () => {
    expect(reasonOf("the proxy hung up")).toBe("the proxy hung up");
    expect(reasonOf(new Error("refused"))).toBe("refused");
  });

  it("names a missing base map, and draws the configured one without a notice", async () => {
    const first = show();
    expect(screen.getByText(s.noBasemap)).toBeInTheDocument();
    first.unmount();
    show(undefined, true, "https://tiles.example.org/style.json");
    expect(screen.queryByText(s.noBasemap)).toBeNull();
  });

  it("says why a type the endpoint refuses is missing and keeps the other", async () => {
    show("PointOfInterest");
    expect(await screen.findByRole("alert")).toHaveTextContent(s.refused(s.type.PointOfInterest, "the policy does not grant this type"));
    await userEvent.click(screen.getByRole("checkbox", { name: new RegExp(`^${s.kind.waste}`) }));
    await waitFor(() => expect(names()).toContain("Korunní 40"));
  });

  it("says it has nothing to read when the app has no endpoint of the city's space", () => {
    show(undefined, false);
    expect(screen.getByRole("status")).toHaveTextContent(s.noEndpoint);
  });

  it("names its sources and has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(names()).toContain("Národní divadlo"));
    expect(screen.getByText(s.attribution)).toBeInTheDocument();
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
