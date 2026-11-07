/**
 * The mobility project's station map over what the shared `helsinki-bikes` endpoint answers (T-3185).
 *
 * `fetch` is what is stubbed, so every case goes through the SDK's own endpoint source. MapLibre
 * needs WebGL, which jsdom does not have, so the library is a double recording what it was given.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import axe from "axe-core";
import { JcProvider, projectRow, toRichRow } from "@joinedcontext/sdk";
import type { EntitySource } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";

const setData = vi.fn();
/** What the map does on a click of a drawn station, as MapLibre would call it. */
let clickStation: ((event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void) | null = null;

vi.mock("maplibre-gl", () => {
  class Map {
    on(event: string, layerOrHandler: unknown, handler?: (event: { features?: Array<{ properties?: Record<string, unknown> }> }) => void) {
      if (event === "load" && typeof layerOrHandler === "function") layerOrHandler();
      if (event === "click" && handler) clickStation = handler;
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    getSource = () => ({ setData });
    remove = vi.fn();
  }
  return { Map };
});
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

const { default: App, loadStations, MOST } = await import("./App");
const { answer } = await import("./fixtures/bikes");
const STATIONS = answer("BikeHireDockingStation") as Array<Record<string, unknown> & { id: string }>;
const { LOCALES } = await import("./locales");
const s = LOCALES.en;

/** The App's own endpoint over its project's space, and the shared one of the helsinki project. */
const OWN = { name: "app-bike-stations", slug: "own7kq3zr5mx2vwt6nbc4pjd7fgs3mob", space: "mobility", types: ["BikeHireDockingStation"] };
const SHARED = { name: "helsinki-bikes", slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf", space: "helsinki", types: [] };

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function show({ refuse = false, endpoints = [OWN, SHARED], language = "en" } = {}) {
  const fetch = vi.fn(async (path: string) => {
    if (refuse) {
      return new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "the policy does not grant this type" }), {
        status: 403,
        headers: { "content-type": "application/problem+json" },
      });
    }
    return json(answer(new URL(path, "http://portal.test").searchParams.get("type")));
  });
  vi.stubGlobal("fetch", fetch);
  // The list reads through `fetch`, the entity panel through the client: the same stations on both.
  const client = stubClient({ entities: STATIONS.map((station) => projectRow(toRichRow(station, "en"), "en")) }, {
    slug: OWN.slug,
    orgDomain: "hel.fi",
    space: "mobility",
    transport: "origin",
    appName: "bike-stations",
    language,
    endpoints,
    portal: "https://portal.hel.fi/projects/helsinki-mobility",
  });
  return {
    fetch,
    ...render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    ),
  };
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  // The handler belongs to the map of the test that drew it; the next test waits for its own.
  clickStation = null;
});

const results = () => screen.getByRole("region", { name: /station|asem/ });
const names = () => within(results()).queryAllByRole("button").map((button) => button.querySelector(".name")?.textContent);

describe("the city bike stations of the mobility project", () => {
  it("reads the stations through the shared endpoint, never the App's own, and counts what is free", async () => {
    const { fetch } = show();
    await waitFor(() => expect(names()).toContain("Kaivopuisto"));
    const asked = fetch.mock.calls.map(([path]) => String(path));
    expect(asked.every((path) => path.includes(`/api/endpoint/${SHARED.slug}/`))).toBe(true);
    expect(names()).toEqual(["Kaivopuisto", "Kalasatama (metro)", "Rautatientori", "Sompasaari", "Töölönlahdenkatu"]);
    // 7 + 0 + 24 + 3 bikes and 23 + 16 + 0 + 9 docks; a station that reports neither adds nothing.
    expect(screen.getByText(s.totals(5, 34, 48))).toBeInTheDocument();
    await waitFor(() => {
      const data = setData.mock.calls.at(-1)?.[0] as { features: unknown[] } | undefined;
      expect(data?.features).toHaveLength(4);
    });
  });

  it("tells an empty, a full and an unreported station apart in words, and marks one with no position", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Töölönlahdenkatu"));
    expect(within(results()).getByRole("button", { name: /Töölönlahdenkatu/ })).toHaveTextContent(s.standing.empty);
    expect(within(results()).getByRole("button", { name: /Rautatientori/ })).toHaveTextContent(s.standing.full);
    expect(within(results()).getByRole("button", { name: /Sompasaari/ })).toHaveTextContent(s.standing.unknown);
    expect(within(results()).getByRole("button", { name: /Kalasatama/ })).toHaveTextContent(s.notOnMap);
  });

  it("searches without diacritics and says when nothing matches", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Kaivopuisto"));
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "toolon");
    expect(names()).toEqual(["Töölönlahdenkatu"]);
    await userEvent.clear(screen.getByRole("searchbox", { name: s.search }));
    await userEvent.type(screen.getByRole("searchbox", { name: s.search }), "nowhere");
    expect(within(results()).getByText(s.noResults)).toBeInTheDocument();
  });

  it("opens a station from the list in the SDK's panel, read through the shared endpoint and linked to its space", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Sompasaari"));
    const item = within(results()).getByRole("button", { name: /Sompasaari/ });
    await userEvent.click(item);
    expect(item).toHaveAttribute("aria-pressed", "true");
    const panel = await screen.findByRole("dialog", { name: "Sompasaari" });
    expect(await within(panel).findByText("closed")).toBeInTheDocument();
    const link = within(panel).getByRole("link", { name: "Open in the Portal" });
    // The station is the helsinki project's: the link names that space, not the App's own.
    expect(link.getAttribute("href")).toContain("space=helsinki&");
    expect(within(panel).queryByRole("button", { name: "Edit" })).toBeNull();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(item).toHaveFocus());
  });

  it("opens a station clicked on the map, and ignores a click on nothing", async () => {
    show();
    await waitFor(() => expect(names()).toContain("Kaivopuisto"));
    // The map binds its click in an effect that may run after the list is on screen: clicking
    // before it would call the last test's map, whose panel is gone (ci-full 38846230558, T-4257).
    await waitFor(() => expect(clickStation).not.toBeNull());
    act(() => clickStation?.({ features: [] }));
    expect(screen.queryByRole("dialog")).toBeNull();
    act(() => clickStation?.({ features: [{ properties: { id: STATIONS[0].id } }] }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it.each([
    ["en", "Close", "Open in the Portal"],
    ["fi", "Sulje", "Avaa portaalissa"],
  ] as const)("in %s, opens every station of the list and searches it", async (language, close, portal) => {
    show({ language });
    await waitFor(() => expect(names()).toHaveLength(5));
    for (const button of within(results()).getAllByRole("button")) {
      fireEvent.click(button);
      const panel = await screen.findByRole("dialog");
      const link = within(panel).getByRole("link", { name: portal });
      link.addEventListener("click", (event) => event.preventDefault());
      fireEvent.click(link);
      fireEvent.click(within(panel).getByRole("button", { name: close }));
    }
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "kaivo" } });
    expect(names()).toEqual(["Kaivopuisto"]);
  });

  it("says why the stations could not be read", async () => {
    show({ refuse: true });
    expect(await screen.findByRole("alert")).toHaveTextContent(s.refused("the policy does not grant this type"));
  });

  it("reads at most two thousand stations, page by page, and says when there are more", async () => {
    const asked: number[] = [];
    const page = (count: number) => Array.from({ length: count }, (_, at) => toRichRow({ ...STATIONS[0], id: `${STATIONS[0].id}-${at}` }));
    const full = { query: async (_q: unknown, at: { offset: number; limit: number }) => (asked.push(at.offset), { rows: page(at.limit) }) } as unknown as EntitySource;
    const loaded = await loadStations(full, "en");
    expect(asked).toHaveLength(MOST / 200);
    expect(loaded).toMatchObject({ status: "ready", truncated: true });
    expect(LOCALES.en.truncated(MOST)).toContain(String(MOST));
    expect(LOCALES.fi.truncated(MOST)).toContain(String(MOST));
  });

  it("says a failure that is no endpoint answer in its own words, and drops an answer after it is gone", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => {
      throw "offline";
    }));
    const config = { slug: OWN.slug, orgDomain: "hel.fi", space: "mobility", transport: "origin" as const, appName: "bike-stations", endpoints: [OWN, SHARED] };
    const first = render(
      <JcProvider client={stubClient(undefined, config)}>
        <App />
      </JcProvider>,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("offline");
    first.unmount();
    let answerLate: (response: Response) => void = () => undefined;
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>((resolve) => (answerLate = resolve))));
    const second = render(
      <JcProvider client={stubClient(undefined, config)}>
        <App />
      </JcProvider>,
    );
    second.unmount();
    const errors = vi.spyOn(console, "error");
    answerLate(json([]));
    await new Promise((settle) => setTimeout(settle, 0));
    expect(errors).not.toHaveBeenCalled();
  });

  it("says it has nothing to read when the shared endpoint is not among its endpoints", () => {
    const { fetch } = show({ endpoints: [OWN] });
    expect(screen.getByText(s.noEndpoint)).toBeInTheDocument();
    expect(fetch).not.toHaveBeenCalled();
  });

  it("speaks Finnish to a Finnish reader", async () => {
    show({ language: "fi-FI" });
    expect(await screen.findByRole("heading", { level: 1, name: LOCALES.fi.title })).toBeInTheDocument();
  });

  it("names its source and has nothing axe finds", async () => {
    const { container } = show();
    await waitFor(() => expect(names()).toContain("Kaivopuisto"));
    expect(screen.getByText(s.attribution)).toBeInTheDocument();
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => v.id)).toEqual([]);
  });
});
