// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/pages/catalogue/CataloguePage.tsx, DatasetPage.tsx, DatasetMap.tsx, search.ts and snippets.ts through the
// router, signed in and anonymous (EP-81, EP-82, T-2726).
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { keyFormats } from "../src/pages/catalogue/CataloguePage";
import { catalogueKey, datasetKey } from "../src/pages/catalogue/queries";
import { DatasetData, dataOrigin, notesBlocks, resourceSays } from "../src/pages/catalogue/DatasetPage";
import { narrowed, parseCatalogueSearch, toggled } from "../src/pages/catalogue/search";
import { snippets } from "../src/pages/catalogue/snippets";
import { DatasetMap, mapCollection } from "../src/pages/catalogue/DatasetMap";
import type { MapLayer } from "../src/components/dashboards/MapLibreView";

const drawn: { layers: MapLayer[]; project?: string } = { layers: [] };
vi.mock("../src/components/dashboards/MapLibreView", () => ({
  MapLibreView: ({ layers, label, project }: { layers: MapLayer[]; label: string; project?: string }) => {
    drawn.layers = layers;
    drawn.project = project;
    return <div role="application" aria-label={label} />;
  },
}));

// The data grid is the SDK EntityGrid behind EndpointDataView, tested on its own (T-6376); here only
// that the dataset page mounts it for its Endpoint.
vi.mock("../src/components/entities/EndpointDataView", () => ({
  EndpointDataView: ({ slug, project, csv, origin }: { slug: string; project: string; csv?: boolean; origin?: string }) => (
    <div data-testid="endpoint-data" data-slug={slug} data-project={project} data-csv={String(Boolean(csv))} data-origin={origin ?? "portal"} />
  ),
}));

const MAP = {
  type: "AirQualityObserved",
  total: 14,
  fields: [
    { slot: "name", label: "Name" },
    { slot: "pm10", label: "PM10" },
  ],
  features: [
    {
      id: "urn:ngsi-ld:AirQualityObserved:a",
      geometry: { type: "Point", coordinates: [14.42, 50.08] },
      properties: { name: "Praha 2 - Legerova", pm10: "21.5" },
    },
    {
      id: "urn:ngsi-ld:AirQualityObserved:b",
      geometry: { type: "Point", coordinates: [14.47, 50.11] },
      properties: { name: "Praha 8 - Karlín" },
    },
  ],
  basemap: "praha",
};

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", roles: ["portal-viewer"] };
const LIST = "joinedcontext.com/v1alpha1";
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const ENDPOINT = `https://api.dev.joinedcontext.com/api/endpoint/${SLUG}/`;

function facet(value: string, label: string, count: number) {
  return { value, label, count };
}

const PAGE = {
  total: 2,
  page: 1,
  pageSize: 20,
  datasets: [
    {
      name: "bbsk-kpi",
      title: "Ukazovatele kraja",
      notes: "Ukazovatele Banskobystrického samosprávneho kraja.",
      publisher: { name: "bbsk", title: "Banskobystrický samosprávny kraj" },
      licence: { id: "cc-by", title: "Creative Commons Attribution" },
      formats: ["CSV", "NGSI-LD"],
      themes: ["ECON"],
      modified: "2026-09-21T05:52:34Z",
    },
    {
      name: "helsinki-bikes",
      title: "City bikes",
      publisher: { name: "helsinki", title: "Helsinki" },
      formats: ["GeoJSON"],
      themes: ["TRAN"],
    },
  ],
  facets: {
    publisher: [facet("bbsk", "Banskobystrický samosprávny kraj", 1), facet("helsinki", "Helsinki", 1)],
    theme: [facet("ECON", "Economy and finance", 1), facet("TRAN", "Transport", 1)],
    format: [facet("CSV", "CSV", 1)],
    licence: [facet("cc-by", "Creative Commons Attribution", 1)],
    spatial: [],
    year: [],
  },
  unavailable: [],
};

const DETAIL = {
  name: "bbsk-kpi",
  title: "Ukazovatele kraja",
  notes: "Ukazovatele Banskobystrického samosprávneho kraja.",
  keywords: ["ukazovatele", "kraj"],
  publisher: { name: "bbsk", title: "Banskobystrický samosprávny kraj" },
  licence: { id: "cc-by", title: "Creative Commons Attribution", url: "https://creativecommons.org/licenses/by/4.0/" },
  frequency: "http://publications.europa.eu/resource/authority/frequency/MONTHLY",
  themes: [{ code: "ECON", label: "Economy and finance" }],
  spatial: ["SK032"],
  temporal: { start: "2020-01-01", end: null },
  catalogueUrl: "https://opendata.dev.joinedcontext.com/dataset/bbsk-kpi",
  resources: [
    {
      name: "CSV",
      format: "CSV",
      url: `${ENDPOINT}file.csv`,
      description: "CSV, generated on each download.\n\n    curl -o file.csv 'https://x/file.csv'",
    },
    { name: "Hostile", format: "HTML", url: "javascript:alert(1)" },
  ],
  endpoint: { url: ENDPOINT, representations: ["ngsi-ld", "csv", "mcp"] },
  model: {
    name: "key-performance-indicator",
    classes: [{ name: "KeyPerformanceIndicator", description: "A measured indicator of the region." }],
    docsUrl: `${ENDPOINT}schema/1/key-performance-indicator.v1.md`,
  },
};

type Answer = { status: number; body: unknown };

describe("the catalogue (EP-81, EP-82)", () => {
  let fetchMock: ReturnType<typeof vi.fn>;
  let answers: Record<string, Answer>;
  let signedIn: boolean;

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    signedIn = true;
    answers = {
      "/api/v1/projects": { status: 200, body: { apiVersion: LIST, kind: "List", items: [{ name: "helsinki" }] } },
      "/api/v1/catalogue": { status: 200, body: PAGE },
      "/api/v1/catalogue/readable": { status: 200, body: { datasets: [], unanswered: 0 } },
      "/api/v1/catalogue/datasets/bbsk-kpi": { status: 200, body: DETAIL },
      "/api/v1/catalogue/datasets/bbsk-kpi/map": { status: 200, body: { ...MAP, total: 0, features: [] } },
    };
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  function renderAt(path: string) {
    window.history.pushState({}, "", path);
    fetchMock = vi.fn((input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
      const answer: Answer = url.pathname.includes("/auth/me")
        ? signedIn
          ? { status: 200, body: IDENTITY }
          : { status: 401, body: { title: "Unauthorized", status: 401 } }
        : (answers[url.pathname] ?? { status: 200, body: { apiVersion: LIST, kind: "List", items: [] } });
      return Promise.resolve(
        new Response(JSON.stringify(answer.body), {
          status: answer.status,
          headers: { "Content-Type": answer.status >= 400 ? "application/problem+json" : "application/json" },
        }),
      );
    });
    vi.stubGlobal("fetch", fetchMock);
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  function catalogueCalls(): URL[] {
    return fetchMock.mock.calls
      .map((call) => new URL((call[0] as Request).url))
      .filter((url) => url.pathname === "/api/v1/catalogue");
  }

  it("lists the datasets with their publisher, themes and formats, and counts them", async () => {
    renderAt("/catalogue");
    expect(await screen.findByRole("heading", { level: 1, name: "Catalogue" })).toBeInTheDocument();
    const results = await screen.findByRole("region", { name: "Datasets" });
    expect(await within(results).findByText("2 datasets")).toBeInTheDocument();
    const link = within(results).getByRole("link", { name: "Ukazovatele kraja" });
    expect(link).toHaveAttribute("href", "/catalogue/bbsk-kpi");
    expect(within(results).getByText("Economy and finance")).toBeInTheDocument();
    expect(within(results).getByText("Transport")).toBeInTheDocument();
    expect(within(results).getByText("NGSI-LD")).toBeInTheDocument();
    // Signed in, the page sits in the Portal's shell and its sidebar marks it.
    expect(screen.getByRole("link", { name: "Catalogue", current: "page" })).toBeInTheDocument();
  });

  it("ends a card in four key formats and counts the rest, which the dataset page lists (T-4995)", async () => {
    const all = ["JSON", "JSON Schema", "JSON-LD", "LinkML", "MCP", "Markdown", "NGSI-LD", "OData", "OGCFEAT", "CSV", "SHACL"];
    answers["/api/v1/catalogue"] = { status: 200, body: { ...PAGE, datasets: [{ ...PAGE.datasets[0], formats: all }] } };
    renderAt("/catalogue");
    const tags = await screen.findByRole("list", { name: "Themes and keywords" });
    const chips = within(tags).getAllByRole("listitem").map((item) => item.textContent);
    expect(chips).toEqual([
      "Economy and finance",
      "CSV",
      "OGCFEAT",
      "NGSI-LD",
      "MCP",
      "+7 formats: JSON, JSON Schema, JSON-LD, LinkML, Markdown, OData, SHACL",
    ]);
  });

  it("opens the filters as a dialog from a button that counts the chosen values (T-4995)", async () => {
    renderAt("/catalogue?publisher=bbsk&theme=ECON");
    const open = await screen.findByRole("button", { name: "Filters (2)" });
    fireEvent.click(open);
    const sheet = await screen.findByRole("dialog", { name: "Filters" });
    expect(within(sheet).getByRole("checkbox", { name: /Banskobystrický samosprávny kraj/ })).toBeChecked();
    fireEvent.click(within(sheet).getByRole("checkbox", { name: /Helsinki/ }));
    await waitFor(() => {
      expect(catalogueCalls().at(-1)?.searchParams.getAll("publisher")).toEqual(["bbsk", "helsinki"]);
    });
    // The sheet stays open over the new answer, and its button closes it onto the list.
    fireEvent.click(await within(screen.getByRole("dialog", { name: "Filters" })).findByRole("button", { name: "Show 2 datasets" }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Filters" })).not.toBeInTheDocument();
    });
    expect(screen.getByRole("button", { name: "Filters (3)" })).toBeInTheDocument();
  });

  it("filters by a facet through the address, and a second value of the same facet is an alternative", async () => {
    renderAt("/catalogue");
    const filters = await screen.findByRole("complementary", { name: "Filters" });
    fireEvent.click(await within(filters).findByRole("checkbox", { name: /Banskobystrický samosprávny kraj/ }));
    await waitFor(() => {
      expect(catalogueCalls().at(-1)?.searchParams.getAll("publisher")).toEqual(["bbsk"]);
    });
    fireEvent.click(within(filters).getByRole("checkbox", { name: /Helsinki/ }));
    await waitFor(() => {
      expect(catalogueCalls().at(-1)?.searchParams.getAll("publisher")).toEqual(["bbsk", "helsinki"]);
    });
    expect(within(filters).getByRole("checkbox", { name: /Helsinki/ })).toBeChecked();
  });

  it("sends the search text and offers to clear it when nothing matches", async () => {
    renderAt("/catalogue");
    const box = await screen.findByRole("searchbox", { name: "Search datasets" });
    answers["/api/v1/catalogue"] = {
      status: 200,
      body: { ...PAGE, total: 0, datasets: [], facets: { ...PAGE.facets, publisher: [], theme: [], format: [], licence: [] } },
    };
    fireEvent.change(box, { target: { value: "  bicykle  " } });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    await waitFor(() => {
      expect(catalogueCalls().at(-1)?.searchParams.get("q")).toBe("bicykle");
    });
    expect(await screen.findByText("No dataset matches")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: "Clear search and filters" })[0]);
    await waitFor(() => {
      expect(catalogueCalls().at(-1)?.searchParams.has("q")).toBe(false);
    });
  });

  it("names the catalogues that did not answer and still lists the others", async () => {
    answers["/api/v1/catalogue"] = { status: 200, body: { ...PAGE, unavailable: ["https://data.example.org"] } };
    renderAt("/catalogue");
    expect(await screen.findByText(/https:\/\/data\.example\.org/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Ukazovatele kraja" })).toBeInTheDocument();
  });

  it("says what the API said when no catalogue answers", async () => {
    answers["/api/v1/catalogue"] = {
      status: 503,
      body: { title: "Service Unavailable", status: 503, detail: "no open-data catalogue answered; try again in a minute" },
    };
    renderAt("/catalogue");
    expect(await screen.findByText(/no open-data catalogue answered/)).toBeInTheDocument();
  });

  it("opens for somebody who is not signed in, with a way to sign in and no project shell", async () => {
    signedIn = false;
    renderAt("/catalogue");
    expect(await screen.findByRole("link", { name: "Ukazovatele kraja" })).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Sign in" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/catalogue");
    const paths = fetchMock.mock.calls.map((call) => new URL((call[0] as Request).url).pathname);
    expect(paths).not.toContain("/api/v1/projects");
  });

  it("shows a dataset with its facts, resources, model, data and the ways to use it", async () => {
    renderAt("/catalogue/bbsk-kpi");
    expect(await screen.findByRole("heading", { level: 1, name: "Ukazovatele kraja" })).toBeInTheDocument();
    expect(screen.getByText("monthly")).toBeInTheDocument();
    expect(screen.getByText("Banskobystrický kraj")).toHaveAttribute("title", "SK032");
    expect(screen.getByRole("link", { name: /Creative Commons Attribution/ })).toHaveAttribute(
      "href",
      "https://creativecommons.org/licenses/by/4.0/",
    );

    const resources = screen.getByRole("table", { name: "Resources" });
    // CKAN holds no rows (T-5060): a resource is a download, never a preview of a copy.
    expect(within(resources).queryByRole("link", { name: /Preview/ })).toBeNull();
    // A resource URL the catalogue holds is checked before it becomes a link.
    // T-4160 #16: a sentence in the page's language; the publisher's commands behind a disclosure.
    expect(within(resources).getByText("A CSV table to download.")).toBeInTheDocument();
    expect(within(resources).getByText(/curl -o file\.csv/).closest("details")).not.toHaveAttribute("open");
    const hostile = within(resources).getByText("Hostile").closest("tr") as HTMLTableRowElement;
    expect(within(hostile).queryByRole("link")).toBeNull();

    expect(screen.getByText("A measured indicator of the region.")).toBeInTheDocument();
    // The entities are the one data grid, read through the Endpoint with the visitor's own session (T-6376).
    expect(screen.getByRole("heading", { level: 2, name: "Data" })).toBeInTheDocument();
    const grid = await screen.findByTestId("endpoint-data");
    expect(grid).toHaveAttribute("data-project", "catalogue");
    expect(grid.getAttribute("data-slug")).toMatch(/^[a-z0-9]{16,64}$/);
    // A signed-in person reads through the Portal host, which carries the session as the bearer.
    expect(grid).toHaveAttribute("data-origin", "portal");

    expect(screen.getByRole("heading", { name: "From a shell (NGSI-LD)" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Download as CSV" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "In an MCP client" })).toBeInTheDocument();
    expect(screen.getByText(/useEntities\("KeyPerformanceIndicator"\)/)).toBeInTheDocument();
    // A snippet scrolls sideways, so the keyboard reaches it and a screen reader names it
    // (axe scrollable-region-focusable, found by the live journey on a real browser).
    const shell = screen.getByLabelText("From a shell (NGSI-LD)");
    expect(shell.tagName).toBe("PRE");
    expect(shell).toHaveAttribute("tabindex", "0");
  });

  it("reads the grid of a visitor without a session from the Endpoint's own data host (T-6540)", async () => {
    signedIn = false;
    renderAt("/catalogue/bbsk-kpi");
    const grid = await screen.findByTestId("endpoint-data");
    // The Portal host would answer each anonymous read with a 308 to this host.
    expect(grid).toHaveAttribute("data-origin", "https://api.dev.joinedcontext.com");
  });

  it("mounts the grid of a signed-in person on the Portal host, and nothing for an address of no Endpoint", async () => {
    const endpoint = { url: "https://api.dev.joinedcontext.com/api/endpoint/abcdefghijklmnopqrst/", representations: ["csv"], feeds: [], calendars: [] };
    const view = render(
      <I18nextProvider i18n={i18n}>
        <DatasetData endpoint={endpoint} />
      </I18nextProvider>,
    );
    const grid = await screen.findByTestId("endpoint-data");
    expect(grid).toHaveAttribute("data-slug", "abcdefghijklmnopqrst");
    expect(grid).toHaveAttribute("data-csv", "true");
    expect(grid).toHaveAttribute("data-origin", "portal");
    expect(screen.getByRole("heading", { level: 2 })).toHaveAttribute("id", "catalogue-data");
    view.rerender(
      <I18nextProvider i18n={i18n}>
        <DatasetData endpoint={{ ...endpoint, url: "https://example.org/other" }} />
      </I18nextProvider>,
    );
    expect(screen.queryByTestId("endpoint-data")).toBeNull();
  });

  it("takes only an https Endpoint origin", () => {
    expect(dataOrigin("https://api.stage.joinedcontext.com/api/endpoint/abc/")).toBe("https://api.stage.joinedcontext.com");
    expect(dataOrigin("http://api.stage.joinedcontext.com/api/endpoint/abc/")).toBeUndefined();
    expect(dataOrigin("javascript:alert(1)")).toBeUndefined();
    expect(dataOrigin("not a url")).toBeUndefined();
  });

  it("tells an unknown or private dataset apart from a failure, and offers the way back", async () => {
    answers["/api/v1/catalogue/datasets/gone"] = {
      status: 404,
      body: { title: "Not Found", status: 404, detail: "dataset 'gone' is not in the catalogue" },
    };
    renderAt("/catalogue/gone");
    expect(await screen.findByText("No public dataset has this name")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "All datasets" })).toHaveAttribute("href", "/catalogue");
  });

  it("draws a dataset's places on a map with labelled popups and the same places as a list (T-5628)", async () => {
    answers["/api/v1/catalogue/datasets/bbsk-kpi/map"] = { status: 200, body: MAP };
    renderAt("/catalogue/bbsk-kpi");
    expect(await screen.findByRole("heading", { level: 2, name: "Places on the map" })).toBeInTheDocument();
    expect(await screen.findByRole("application", { name: "Map of AirQualityObserved" })).toBeInTheDocument();
    expect(screen.getByText("The map shows the first 2 of 14 places.")).toBeInTheDocument();
    const [layer] = drawn.layers;
    expect(layer.popupProperties).toEqual(["Name", "PM10"]);
    expect(layer.cluster).toBe(true);
    expect(drawn.project).toBe("praha");
    expect((layer.data as { features: { properties: unknown }[] }).features[0].properties).toEqual({
      Name: "Praha 2 - Legerova",
      PM10: "21.5",
    });
    fireEvent.click(screen.getByText("The 2 places as a list"));
    const list = screen.getByRole("table", { name: "Map of AirQualityObserved" });
    expect(within(list).getByRole("columnheader", { name: "PM10" })).toBeInTheDocument();
    expect(within(list).getByText("Praha 8 - Karlín")).toBeInTheDocument();
    expect(within(list).getByText("—")).toBeInTheDocument();
    const asked = fetchMock.mock.calls
      .map((call) => new URL((call[0] as Request).url))
      .find((url) => url.pathname.endsWith("/map"));
    expect(asked?.searchParams.get("lang")).toBe("en");
  });

  it("says the map shows the first places when the type was too big to count in time (T-6541)", async () => {
    answers["/api/v1/catalogue/datasets/bbsk-kpi/map"] = { status: 200, body: { ...MAP, total: null } };
    renderAt("/catalogue/bbsk-kpi");
    expect(await screen.findByText("The map shows the first 2 places; the dataset holds more.")).toBeInTheDocument();
  });

  it("asks for the map's labels in the page's language", async () => {
    await i18n.changeLanguage("sk");
    answers["/api/v1/catalogue/datasets/bbsk-kpi/map"] = { status: 200, body: { ...MAP, total: 2 } };
    renderAt("/catalogue/bbsk-kpi");
    expect(await screen.findByText("2 miesta na mape.")).toBeInTheDocument();
    const asked = fetchMock.mock.calls
      .map((call) => new URL((call[0] as Request).url))
      .find((url) => url.pathname.endsWith("/map"));
    expect(asked?.searchParams.get("lang")).toBe("sk");
  });

  it("shows no map and no empty box for a dataset without places", async () => {
    renderAt("/catalogue/bbsk-kpi");
    expect(await screen.findByTestId("endpoint-data")).toBeInTheDocument();
    await waitFor(() =>
      expect(fetchMock.mock.calls.map((call) => new URL((call[0] as Request).url).pathname)).toContain(
        "/api/v1/catalogue/datasets/bbsk-kpi/map",
      ),
    );
    expect(screen.queryByRole("heading", { name: "Places on the map" })).toBeNull();
    expect(screen.queryByRole("application")).toBeNull();
  });

  it("says why when the map cannot be read, and reads it again on retry", async () => {
    answers["/api/v1/catalogue/datasets/bbsk-kpi/map"] = {
      status: 503,
      body: { title: "Service Unavailable", status: 503, detail: "The endpoint of dataset 'bbsk-kpi' did not answer; try again in a minute." },
    };
    renderAt("/catalogue/bbsk-kpi");
    expect(await screen.findByRole("heading", { level: 2, name: "Places on the map" })).toBeInTheDocument();
    expect(await screen.findByRole("alert")).toHaveTextContent(/The endpoint of dataset 'bbsk-kpi' did not answer; try again in a minute\.Retry$/);
    answers["/api/v1/catalogue/datasets/bbsk-kpi/map"] = { status: 200, body: MAP };
    fireEvent.click(within(screen.getByRole("alert")).getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("application", { name: "Map of AirQualityObserved" })).toBeInTheDocument();
  });

  it("shows no data grid and no snippets for a dataset of no endpoint here", async () => {
    answers["/api/v1/catalogue/datasets/bbsk-kpi"] = {
      status: 200,
      body: { ...DETAIL, endpoint: undefined, model: undefined },
    };
    renderAt("/catalogue/bbsk-kpi");
    expect(await screen.findByRole("heading", { level: 1, name: "Ukazovatele kraja" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Data" })).toBeNull();
    expect(screen.queryByTestId("endpoint-data")).toBeNull();
    expect(screen.queryByRole("heading", { name: "Use this data" })).toBeNull();
    const paths = fetchMock.mock.calls.map((call) => new URL((call[0] as Request).url).pathname);
    expect(paths).not.toContain("/api/v1/catalogue/datasets/bbsk-kpi/sample");
  });
});

describe("the dataset map on its own (T-5628)", () => {
  function renderMap(answer: Answer) {
    const fetchMock = vi.fn(() =>
      Promise.resolve(
        new Response(JSON.stringify(answer.body), {
          status: answer.status,
          headers: { "Content-Type": answer.status >= 400 ? "application/problem+json" : "application/json" },
        }),
      ),
    );
    vi.stubGlobal("fetch", fetchMock);
    const view = render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <DatasetMap name="bbsk-kpi" />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    return { fetchMock, view };
  }

  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("draws the places it is given and counts them", async () => {
    renderMap({ status: 200, body: MAP });
    expect(await screen.findByRole("application", { name: "Map of AirQualityObserved" })).toBeInTheDocument();
    expect(screen.getByText("The map shows the first 2 of 14 places.")).toBeInTheDocument();
    expect(drawn.layers[0].popupProperties).toEqual(["Name", "PM10"]);
  });

  it("draws the map only once its frame comes into sight (T-6600)", async () => {
    const seen: ((entries: { isIntersecting: boolean }[]) => void)[] = [];
    vi.stubGlobal(
      "IntersectionObserver",
      class {
        constructor(callback: (entries: { isIntersecting: boolean }[]) => void) {
          seen.push(callback);
        }
        observe() {}
        disconnect() {}
      },
    );
    renderMap({ status: 200, body: MAP });
    expect(await screen.findByText("The map shows the first 2 of 14 places.")).toBeInTheDocument();
    expect(screen.getByText("Loading the map…")).toBeInTheDocument();
    // The lazy map module loads in a tick: wait past it, the frame still out of sight.
    await expect(screen.findByRole("application", {}, { timeout: 300 })).rejects.toThrow();
    act(() => seen.forEach((callback) => callback([{ isIntersecting: false }])));
    await expect(screen.findByRole("application", {}, { timeout: 300 })).rejects.toThrow();
    act(() => seen.forEach((callback) => callback([{ isIntersecting: true }])));
    expect(await screen.findByRole("application", { name: "Map of AirQualityObserved" })).toBeInTheDocument();
  });

  it("renders nothing for a dataset without places or without an endpoint", async () => {
    for (const answer of [
      { status: 200, body: { ...MAP, total: 0, features: [] } },
      { status: 404, body: { title: "Not Found", status: 404, detail: "dataset 'bbsk-kpi' has no public endpoint here" } },
    ]) {
      const { fetchMock, view } = renderMap(answer);
      await waitFor(() => expect(fetchMock).toHaveBeenCalled());
      await waitFor(() => expect(view.container).toBeEmptyDOMElement());
      view.unmount();
    }
  });

  it("says in one sentence why the map cannot be read, with a generic line when the server gives none", async () => {
    const { view } = renderMap({
      status: 503,
      body: { title: "Service Unavailable", status: 503, detail: "The endpoint of dataset 'bbsk-kpi' did not answer; try again in a minute." },
    });
    expect(await screen.findByRole("alert")).toHaveTextContent(/The endpoint of dataset 'bbsk-kpi' did not answer; try again in a minute\.Retry$/);
    expect(screen.getByRole("alert")).not.toHaveTextContent("could not be read");
    view.unmount();
    renderMap({ status: 503, body: { title: "Service Unavailable", status: 503 } });
    expect(await screen.findByRole("alert")).toHaveTextContent(/The map could not be read; try again in a moment\.Retry$/);
  });
});

describe("the dataset map's features (T-5628)", () => {
  it("keys each popup line by its label and leaves out what an entity does not hold", () => {
    const [first, second] = mapCollection(MAP as unknown as Parameters<typeof mapCollection>[0]).features as { properties: Record<string, string> }[];
    expect(first.properties).toEqual({ Name: "Praha 2 - Legerova", PM10: "21.5" });
    expect(second.properties).toEqual({ Name: "Praha 8 - Karlín" });
  });
});

describe("the catalogue's search in the address", () => {
  it("keeps what it knows and drops the rest", () => {
    expect(
      parseCatalogueSearch({ q: "  ", publisher: ["bbsk", "bbsk", ""], theme: "ECON", page: "0", other: "x" }),
    ).toEqual({ publisher: ["bbsk"], theme: ["ECON"] });
    expect(parseCatalogueSearch({ q: "x".repeat(300), page: 3 })).toEqual({ q: "x".repeat(200), page: 3 });
    expect(parseCatalogueSearch({ page: "2.5", year: [2024] })).toEqual({});
  });

  it("puts the formats a person picks first and keeps four", () => {
    expect(keyFormats(["SHACL", "MCP", "OWL", "CSV", "RDF", "OData"])).toEqual(["CSV", "MCP", "SHACL", "OWL"]);
    expect(keyFormats(["GeoJSON"])).toEqual(["GeoJSON"]);
    expect(keyFormats([])).toEqual([]);
  });

  it("turns a value on and off and starts again at page one", () => {
    const on = toggled({ page: 3 }, "format", "CSV");
    expect(on).toEqual({ format: ["CSV"], page: undefined });
    expect(toggled(on, "format", "CSV")).toEqual({ format: undefined, page: undefined });
    expect(narrowed({})).toBe(false);
    expect(narrowed({ year: ["2025"] })).toBe(true);
  });
});

describe("the catalogue's cache", () => {
  it("keeps one answer per search and one per dataset, all under the catalogue", () => {
    expect(catalogueKey({ q: "a" })).not.toEqual(catalogueKey({ q: "b" }));
    expect(catalogueKey({})[0]).toBe("catalogue");
    expect(datasetKey("bbsk-kpi")).toEqual(["catalogue", "datasets", "bbsk-kpi"]);
  });
});

describe("use this data", () => {
  it("offers only what the endpoint serves, and quotes what a shell would split", () => {
    const all = snippets(ENDPOINT, ["ngsi-ld", "csv", "mcp"], "Bike Station", "helsinki bikes");
    expect(all.map((s) => s.key)).toEqual(["curl", "sdk", "csv", "mcp"]);
    expect(all[0].code).toBe(
      `curl -H 'Accept: application/json' '${ENDPOINT}ngsi-ld/v1/entities?type=Bike+Station&limit=10&options=keyValues'`,
    );
    expect(JSON.parse(all[3].code)).toEqual({
      mcpServers: { "helsinki-bikes": { type: "http", url: `${ENDPOINT}mcp` } },
    });
    expect(snippets(ENDPOINT, ["geojson"], "T", "d")).toEqual([]);
    // NGSI-LD without a known class has nothing to name in a query.
    expect(snippets(ENDPOINT, ["ngsi-ld"], undefined, "d")).toEqual([]);
    expect(snippets("https://h/api/endpoint/x", ["csv"], undefined, "d")[0].code).toBe(
      "curl -o data.csv 'https://h/api/endpoint/x/file.csv'",
    );
    expect(snippets("https://h/it's/", ["csv"], undefined, "d")[0].code).toBe(
      `curl -o data.csv 'https://h/it'"'"'s/file.csv'`,
    );
  });
});

describe("what a resource is, in words (T-4264, T-4160 #16)", () => {
  it("says each published format in the page's language, a model formalism by name, nothing for an unknown one", async () => {
    await i18n.changeLanguage("sk");
    const t = (key: string, options?: Record<string, unknown>) => i18n.t(key, options);
    expect(resourceSays("CSV", t)).toBe("Tabuľka CSV na stiahnutie.");
    expect(resourceSays("NGSI-LD", t)).toBe("Dopytovanie a filtrovanie entít cez NGSI-LD API.");
    expect(resourceSays("JSON Schema", t)).toBe("Dátový model vo formáte JSON Schema.");
    expect(resourceSays("HTML", t)).toBeNull();
    expect(resourceSays("", t)).toBeNull();
    await i18n.changeLanguage("en");
  });
});

describe("the publisher's Markdown, in words and code (T-4949)", () => {
  it("splits paragraphs from indented code, so JSON and commands land in code", () => {
    const text =
      "Live updates: open one WebSocket. With `websocat`, then type the subscription:\n\n" +
      "    websocat --protocol ngsi-ld-ws 'wss://api.example/ws'\n" +
      '    {"metadata":{"x-op":"x-subscribe"}}\n\n' +
      "The message format: https://docs.example/API/07-websocket";
    expect(notesBlocks(text)).toEqual([
      { code: false, text: "Live updates: open one WebSocket. With `websocat`, then type the subscription:" },
      { code: true, text: "websocat --protocol ngsi-ld-ws 'wss://api.example/ws'\n{\"metadata\":{\"x-op\":\"x-subscribe\"}}" },
      { code: false, text: "The message format: https://docs.example/API/07-websocket" },
    ]);
    expect(notesBlocks("")).toEqual([]);
    expect(notesBlocks("One line.\r\n\r\n\tcurl x")).toEqual([
      { code: false, text: "One line." },
      { code: true, text: "curl x" },
    ]);
  });
});
