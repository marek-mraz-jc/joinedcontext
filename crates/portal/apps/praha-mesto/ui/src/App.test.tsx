/**
 * The screen over the rows the praha pipelines write (T-2917). `fetch` is what is stubbed and
 * nothing below it, so every case goes through the SDK's own endpoint source.
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App, { reasonOf } from "./App";
import bikes from "./fixtures/bikes.json";
import parking from "./fixtures/parking.json";
import air from "./fixtures/air.json";
import { LOCALES } from "./locales";
import { drawn } from "../test-setup";

const SLUG = "k3pq7vwyxz2a4b5c6d7e8f9g0h1j2m3n";
const ENDPOINTS = [{ name: "app-praha-mesto", slug: SLUG, space: "praha-mesto", types: ["BikeHireDockingStation"] }];

const answer = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const BY_TYPE: Record<string, unknown[]> = {
  BikeHireDockingStation: bikes,
  OffStreetParking: parking,
  AirQualityObserved: air,
};

/** Answers each type with its rows; `override` replaces one type's answer. */
function serving(override: Record<string, () => Response> = {}) {
  return vi.fn(async (path: string) => {
    const url = new URL(path, "https://portal.example");
    const type = url.searchParams.get("type") ?? "";
    if (!url.pathname.includes(`/api/endpoint/${SLUG}/`)) throw new Error(`no stub for ${path}`);
    if (override[type]) return override[type]();
    return answer(BY_TYPE[type] ?? []);
  });
}

function show(options: { override?: Record<string, () => Response>; language?: string; endpoints?: typeof ENDPOINTS | null; basemap?: string } = {}) {
  const fetch = serving(options.override);
  vi.stubGlobal("fetch", fetch);
  // The tables read through `fetch` above; the entity panel reads one entity through the client (SDK-40).
  const rows = [...bikes, ...parking, ...air] as unknown as Row[];
  const client = stubClient({ entities: rows }, {
    portal: "https://portal.praha.cz/projects/praha",
    slug: SLUG,
    orgDomain: "praha.cz",
    space: "praha-mesto",
    transport: "origin",
    appName: "praha-mesto",
    language: options.language ?? "cs",
    ...(options.endpoints === null ? {} : { endpoints: options.endpoints ?? ENDPOINTS }),
    basemap: options.basemap,
  });
  const view = render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return Object.assign(fetch, { view });
}

const section = (name: string) => screen.getByRole("region", { name });
const cs = LOCALES.cs;

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Prague right now", () => {
  it("draws the stations on the served basemap in the ramp's colours, with a legend and a chart", async () => {
    drawn.data.length = 0;
    drawn.style.length = 0;
    const located = bikes.map((entity, i) => ({
      ...entity,
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [14.4 + i / 100, 50.07] } },
    }));
    show({ basemap: "https://tiles.example/style.json", override: { BikeHireDockingStation: () => answer(located) } });
    const bikesSection = section(cs.bikes.title);
    // A draw before the stations are read carries none; wait for the one that carries them (T-2994).
    type Drawn = { features: { geometry: { coordinates: number[] }; properties: { colour: string } }[] };
    await waitFor(() => expect((drawn.data.at(-1) as Drawn | undefined)?.features).toHaveLength(3));
    expect(drawn.style).toEqual(["https://tiles.example/style.json"]);
    const last = drawn.data.at(-1) as Drawn;
    // Three named stations in service, each at its own point, coloured by its step.
    expect(last.features).toHaveLength(3);
    expect(new Set(last.features.map((feature) => feature.properties.colour)).size).toBe(2);
    expect(within(bikesSection).getAllByRole("listitem").filter((item) => item.closest(".legend"))).toHaveLength(5);
    expect(await within(bikesSection).findByRole("img", { name: /0: 2, 1–2: 1/ })).toBeInTheDocument();
    await waitFor(() => expect(within(section(cs.parking.title)).getByRole("img", { name: cs.parking.chart(2) })).toBeInTheDocument());
    await waitFor(() => expect(within(section(cs.air.title)).getByRole("img", { name: cs.air.chart(2) })).toBeInTheDocument());
  });

  it("shows the bikes, the car parks and the air from the space, each under its own heading", async () => {
    show();
    const bikesSection = section(cs.bikes.title);
    await waitFor(() => expect(within(bikesSection).getByRole("rowheader", { name: "P10-Čechovo náměstí" })).toBeInTheDocument());
    // Three named stations in service: one bike to rent, 29 free docks.
    expect(within(bikesSection).getByText("29").closest("li")).toHaveTextContent(cs.bikes.docks);
    const parks = section(cs.parking.title);
    await waitFor(() => expect(within(parks).getByRole("rowheader", { name: "P+R Běchovice" })).toBeInTheDocument());
    const counted = within(parks).getByRole("rowheader", { name: "P+R Běchovice" }).closest("tr");
    expect(counted).toHaveTextContent("92");
    expect(counted).toHaveTextContent("23");
    expect(within(parks).getByRole("rowheader", { name: "P+R Černý Most II" }).closest("tr")).toHaveTextContent(cs.parking.noLive);
    const airSection = section(cs.air.title);
    await waitFor(() => expect(within(airSection).getByRole("rowheader", { name: "Praha 4-Libuš" })).toBeInTheDocument());
    expect(within(airSection).getByRole("rowheader", { name: "Praha 4-Libuš" }).closest("tr")).toHaveTextContent("4,9");
  });

  it("narrows the stations by a search typed without diacritics", async () => {
    show();
    const bikesSection = section(cs.bikes.title);
    await waitFor(() => expect(within(bikesSection).getAllByRole("row")).toHaveLength(4));
    fireEvent.change(within(bikesSection).getByLabelText(cs.search), { target: { value: "vrsovicke" } });
    expect(within(bikesSection).getAllByRole("row")).toHaveLength(2);
    expect(within(bikesSection).getByText(cs.shown(1, 3))).toBeInTheDocument();
  });

  it("keeps the other sections when one type is refused, and says what failed in its place", async () => {
    show({
      override: {
        OffStreetParking: () =>
          new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "the endpoint does not serve OffStreetParking" }), {
            status: 403,
            headers: { "content-type": "application/problem+json" },
          }),
      },
    });
    const parks = section(cs.parking.title);
    await waitFor(() => expect(within(parks).getByRole("alert")).toHaveTextContent(cs.unavailable));
    await waitFor(() => expect(within(section(cs.air.title)).getByText("Praha 4-Libuš")).toBeInTheDocument());
  });

  it("says so when the space is empty and when the app has no endpoint for it", async () => {
    show({ override: { AirQualityObserved: () => answer([]) } });
    await waitFor(() => expect(within(section(cs.air.title)).getByText(cs.empty)).toBeInTheDocument());
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
    const fetch = show({ endpoints: [] });
    await waitFor(() => expect(screen.getAllByText(cs.noEndpoint)).toHaveLength(3));
    expect(fetch).not.toHaveBeenCalled();
  });

  it("reads every page of a type the endpoint pages", async () => {
    const many = Array.from({ length: 501 }, (_, i) => ({ ...bikes[0], id: `${bikes[0].id}${i}` }));
    const fetch = show({
      override: {
        BikeHireDockingStation: (() => {
          let call = 0;
          return () => answer(call++ === 0 ? many.slice(0, 500) : many.slice(500));
        })(),
      },
    });
    await waitFor(() => expect(within(section(cs.bikes.title)).getByText(cs.shown(50, 501))).toBeInTheDocument());
    const asked = fetch.mock.calls.map(([path]) => new URL(path, "https://portal.example")).filter((url) => url.searchParams.get("type") === "BikeHireDockingStation");
    expect(asked.map((url) => Number(url.searchParams.get("offset") ?? 0))).toEqual([0, 500]);
  });

  it("speaks English when the Portal does, and has no accessibility violation", async () => {
    show({ language: "en" });
    await waitFor(() => expect(screen.getByRole("heading", { level: 1, name: LOCALES.en.title })).toBeInTheDocument());
    await waitFor(() => expect(within(section(LOCALES.en.air.title)).getByText("Praha 4-Libuš")).toBeInTheDocument());
    const result = await axe.run(document.body);
    expect(result.violations.map((violation) => violation.id)).toEqual([]);
  });

  // T-3045: on a phone each table scrolls sideways in its own box, which axe on dev found out of
  // keyboard reach (scrollable-region-focusable). jsdom lays nothing out, so what is held is that
  // each box takes focus and is named by its section's heading.
  it("lets the keyboard reach and scroll every table, each named by its section", async () => {
    show({ language: "en" });
    const titles = [LOCALES.en.bikes.title, LOCALES.en.parking.title, LOCALES.en.air.title];
    for (const title of titles) {
      await waitFor(() => expect(within(section(title)).getByRole("table")).toBeInTheDocument());
      const box = within(section(title)).getByRole("group", { name: title });
      expect(box).toHaveAttribute("tabindex", "0");
      expect(within(box).getByRole("table")).toBeInTheDocument();
    }
  });

  it("opens a station, a car park and an air station in the entity panel, with a Portal link and no Edit", async () => {
    show();
    for (const [heading, kind] of [
      [cs.bikes.title, "bike"],
      [cs.parking.title, "parking"],
      [cs.air.title, "air"],
    ] as const) {
      const table = await within(section(heading)).findByRole("table");
      const first = within(table).getAllByRole("button")[0];
      fireEvent.click(first);
      const panel = await screen.findByRole("dialog");
      await waitFor(() => expect(within(panel).getByRole("link", { name: "Otevřít v Portálu" })).toBeInTheDocument(), { timeout: 3000 });
      expect(within(panel).queryByRole("button", { name: "Upravit" })).toBeNull();
      fireEvent.click(within(panel).getByRole("button", { name: "Zavřít" }));
      expect(screen.queryByRole("dialog"), kind).toBeNull();
    }
  });

  it("opens every row of every table, and links the opened one to the Portal", async () => {
    show();
    for (const heading of [cs.bikes.title, cs.parking.title, cs.air.title]) {
      const table = await within(section(heading)).findByRole("table");
      for (const button of within(table).getAllByRole("button")) {
        fireEvent.click(button);
        expect(await screen.findByRole("dialog")).toBeInTheDocument();
      }
    }
    const link = await within(screen.getByRole("dialog")).findByRole("link", { name: "Otevřít v Portálu" }, { timeout: 3000 });
    fireEvent.click(link);
    expect(link).toHaveAttribute("href", expect.stringContaining("https://portal.praha.cz/projects/praha/explore?"));
  });

  it("finds a station by an English search too", async () => {
    show({ language: "en" });
    const bikesSection = section(LOCALES.en.bikes.title);
    await waitFor(() => expect(within(bikesSection).getAllByRole("row")).toHaveLength(4));
    fireEvent.change(within(bikesSection).getByLabelText(LOCALES.en.search), { target: { value: "cechovo" } });
    expect(within(bikesSection).getAllByRole("row")).toHaveLength(2);
  });

  it("shows a dash for a count, a capacity or a reading a row does not carry", async () => {
    const bare = { ...bikes[0], availableBikeNumber: undefined, freeSlotNumber: undefined, status: { type: "Property", value: "outOfService" } };
    const park = { ...parking[1], totalSpotNumber: undefined, availableSpotNumber: undefined, occupiedSpotNumber: { type: "Property", value: 5 }, dateModified: undefined };
    const station = { id: air[0].id, type: "AirQualityObserved", name: air[0].name, no2: { type: "Property", value: 12 } };
    show({
      override: {
        BikeHireDockingStation: () => answer([bare, ...bikes.slice(1)]),
        OffStreetParking: () => answer([park]),
        AirQualityObserved: () => answer([station]),
      },
    });
    const bikeRow = (await within(section(cs.bikes.title)).findByRole("rowheader", { name: new RegExp(cs.outOfService) })).closest("tr");
    expect(bikeRow).toHaveTextContent("–");
    const parkRow = (await within(section(cs.parking.title)).findByRole("rowheader")).closest("tr");
    expect(parkRow?.textContent?.match(/–/g)?.length).toBeGreaterThanOrEqual(3);
    const airRow = (await within(section(cs.air.title)).findByRole("rowheader")).closest("tr");
    expect(airRow?.textContent?.match(/–/g)?.length).toBe(4);
  });

  it("says it cannot reach the space when the served configuration lists no endpoints", async () => {
    const fetch = show({ endpoints: null });
    await waitFor(() => expect(screen.getAllByText(cs.noEndpoint)).toHaveLength(3));
    expect(fetch).not.toHaveBeenCalled();
  });

  it("drops what arrives after the screen is gone", async () => {
    let release: () => void = () => {};
    const held = new Promise<void>((done) => (release = done));
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => {
        await held;
        return answer(BY_TYPE[new URL(path, "https://portal.example").searchParams.get("type") ?? ""] ?? []);
      }),
    );
    const client = stubClient(undefined, { slug: SLUG, orgDomain: "praha.cz", space: "praha-mesto", transport: "origin", appName: "praha-mesto", language: "cs", endpoints: ENDPOINTS });
    const view = render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    view.unmount();
    release();
    await new Promise((done) => setTimeout(done, 20));
    expect(document.body.textContent).toBe("");
  });

  it("says a failure that is not even an error as it came", () => {
    expect(reasonOf("the proxy hung up")).toBe("the proxy hung up");
    expect(reasonOf(new Error("refused"))).toBe("refused");
  });
});
