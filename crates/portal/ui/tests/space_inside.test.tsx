import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { inTurn, refusalKey, Samples } from "../src/pages/spaces/SpaceInside";
import { entityTypesOf, parseResultsCount, pickReadEndpoint, spaceOf } from "../src/pages/spaces/spaceFacts";
import type { Manifest } from "../src/api/manifest";
import { ApiError } from "../src/api/client";
import { spaceUsageQuery, usageRefusal } from "../src/api/spaceUsage";
import { readLayout, writeLayout } from "../src/components/entities/viewLayout";
import { pageFindings } from "./pageChecks";
import { renderPart } from "./page_contract";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

/** Opens one tab of the space's page (T-4825: the page lands on the rows). */
async function openTab(name: string) {
  await userEvent.click(await screen.findByRole("tab", { name }));
}

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

function list(items: unknown[]) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items };
}

function manifest(kind: string, name: string, spec: Record<string, unknown>): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind,
    metadata: { name, namespace: "banskabystrica" },
    spec,
  } as Manifest;
}

const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: {
    name: "ovzdusie",
    namespace: "banskabystrica",
    title: { sk: "Ovzdušie", en: "Air quality" },
  },
  spec: { dataModelRef: "bb-air-quality" },
  status: { phase: "Live" },
};

const MODELS = list([
  manifest("DataModel", "bb-air-quality", {
    linkml: "./bb-air-quality.linkml.yaml",
    version: "2.1.0",
    classes: ["AirQualityObserved", "AirQualityStation"],
    artifacts: { jsonSchema: "./bb-air-quality.v2.schema.json" },
  }),
]);

const ENDPOINTS = list([
  {
    ...manifest("Endpoint", "public-air", {
      contextSpaceRef: "ovzdusie",
      slug: SLUG,
      audience: "public",
      enabledRepresentations: [
        "ngsi-ld",
        "geojson",
        "csv",
        "xlsx",
        "zip",
        "mcp",
        "linkml",
        "jsonschema",
        "sensorthings",
        "ogc-features",
        "dcat",
      ],
      policyRef: "urn:ngsi-ld:Policy:banskabystrica.sk:ovzdusie:public-air-quality",
      publish: { ckan: { instanceRef: { kind: "CkanInstance", name: "bb-open-data" } } },
    }),
    status: { phase: "Live" },
  },
  manifest("Endpoint", "other-space", {
    contextSpaceRef: "doprava",
    slug: "zzzzzzzzzzzzzzzzzzzzzzzzzz",
    audience: "public",
    enabledRepresentations: ["ngsi-ld"],
  }),
]);

/** The catalogue `public-air` publishes to: its link is this site, not one guessed (T-3018). */
const CATALOGUES = list([
  manifest("CkanInstance", "bb-open-data", {
    url: "https://data.city.example/",
    secretRef: { name: "ckan-token" },
  }),
]);

const POLICIES = list([
  manifest("Policy", "public-air-quality", {
    contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
    assignee: { kind: "role", id: "public" },
    operations: ["queryEntity", "retrieveEntity"],
    information: [{ entities: [{ type: "AirQualityObserved" }] }],
  }),
]);

const SAMPLES = [
  {
    id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:1",
    type: "AirQualityObserved",
    pm10: 12,
    name: { languageMap: { sk: "Námestie SNP", en: "SNP Square" } },
    location: { type: "Point", coordinates: [19.14606, 48.73562] },
  },
  { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:2", type: "AirQualityObserved", pm10: 9 },
];

/** One call, whichever way it was made: the page sends a `Request`, the SDK grid a path (T-1434). */
function urlOf(input: unknown): URL {
  const raw = input instanceof Request ? input.url : String(input);
  return new URL(raw, window.location.origin);
}

/** The entities of the space surface, as `/cs/{space}/…` answers them for this caller (SP-06). */
const SPACE_ROWS = [
  {
    id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:1",
    type: "AirQualityObserved",
    pm10: { type: "Property", value: 12, unitCode: "GQ" },
  },
];

function renderInside(
  gateway: { status: number; count?: number },
  surface: {
    status: number;
    rows?: unknown[];
    answer?: (url: URL) => unknown[];
    holdProbe?: boolean;
    headers?: Record<string, string>;
  } = { status: 200 },
  seed: {
    space?: unknown;
    models?: unknown;
    endpoints?: unknown;
    usage?: { status: number; body: unknown };
    /** The model's committed LinkML source, which the space's data reads for its field rules. */
    source?: string;
    /** The saved data views of the space (API/01 §30). */
    views?: unknown[];
    /** What the endpoint's gateway answers for the caller's grants (EP-55). */
    access?: unknown;
    /** Its decisions per attribute (T-5528). */
    attributes?: unknown;
  } = {},
) {
  const fetchMock = vi.fn((input: RequestInfo | URL, _init?: RequestInit) => {
    const url = urlOf(input);
    const path = url.pathname;
    const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json", ...headers },
        }),
      );

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path === `/api/endpoint/${SLUG}/access/attributes`) {
      return json(seed.attributes ?? { type: url.searchParams.get("type"), attributes: [] });
    }
    if (path === `/api/endpoint/${SLUG}/access`) {
      return json(seed.access ?? { permissions: [] });
    }
    if (path.startsWith(`/api/endpoint/${SLUG}/ngsi-ld/v1/entities`)) {
      if (gateway.status !== 200) {
        return json({ title: "Forbidden" }, gateway.status);
      }
      const limit = Number(url.searchParams.get("limit") ?? SAMPLES.length);
      return json(SAMPLES.slice(0, limit), 200, url.searchParams.get("count") === "true" ? { "NGSILD-Results-Count": String(gateway.count ?? 0) } : {});
    }
    // The space's own surface: the whole space, by the caller's grants (T-1434).
    if (path.startsWith("/cs/ovzdusie/ngsi-ld/v1/entities")) {
      if (surface.holdProbe && url.searchParams.get("limit") === "1") {
        return new Promise<Response>(() => undefined);
      }
      // Status 0: the request never reached an answer (the network or the edge dropped it).
      if (surface.status === 0) throw new TypeError("Failed to fetch");
      if (surface.status !== 200) {
        return json({ title: "Not Found" }, surface.status, surface.headers);
      }
      const answered = surface.answer?.(url) ?? surface.rows ?? SPACE_ROWS;
      return json(answered, 200, { "NGSILD-Results-Count": String(answered.length) });
    }
    if (path.endsWith("/spaces/ovzdusie/usage")) {
      const usage = seed.usage ?? { status: 200, body: { entities: 0, broker: "main", observedAt: "2026-09-25T11:40:00Z" } };
      return json(usage.body, usage.status);
    }
    if (path.endsWith("/spaces/ovzdusie")) {
      return json(seed.space ?? SPACE);
    }
    if (path.endsWith("/spaces/ovzdusie/views")) {
      return json({ items: seed.views ?? [] });
    }
    if (path.endsWith("/datamodels/bb-air-quality/source") && seed.source !== undefined) {
      return Promise.resolve(new Response(seed.source, { status: 200, headers: { "Content-Type": "text/yaml" } }));
    }
    if (path.endsWith("/datamodels")) {
      return json(seed.models ?? MODELS);
    }
    if (path.endsWith("/endpoints")) {
      return json(seed.endpoints ?? ENDPOINTS);
    }
    if (path.endsWith("/ckaninstances")) {
      return json(CATALOGUES);
    }
    if (path.endsWith("/policies")) {
      return json(POLICIES);
    }
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe("a space's usage (T-2889)", () => {
  it("is kept as long as the Portal keeps it and a refusal is not retried", () => {
    const query = spaceUsageQuery("banskabystrica", "ovzdusie");
    expect(query.queryKey).toEqual(["space-usage", "banskabystrica", "ovzdusie"]);
    expect(query.staleTime).toBe(5 * 60 * 1000);
    expect(query.retry).toBe(false);
  });

  it("gives the server's reason, then the error's own words", () => {
    const refused = new ApiError(503, "Service Unavailable", {
      type: "about:blank",
      title: "Service Unavailable",
      status: 503,
      detail: "the broker answered 500",
    });
    expect(usageRefusal(refused)).toBe("the broker answered 500");
    expect(usageRefusal(new Error("offline"))).toBe("offline");
    expect(usageRefusal("nothing")).toBe("");
  });
});

describe("count header parsing", () => {
  it("reads NGSILD-Results-Count as a number, case-insensitively", () => {
    expect(parseResultsCount(new Headers({ "NGSILD-Results-Count": "42" }))).toBe(42);
    expect(parseResultsCount(new Headers({ "ngsild-results-count": " 7 " }))).toBe(7);
    expect(parseResultsCount(new Headers({ "NGSILD-Results-Count": "0" }))).toBe(0);
  });

  it("answers undefined for a missing, empty or non-numeric header", () => {
    expect(parseResultsCount(new Headers())).toBeUndefined();
    expect(parseResultsCount(new Headers({ "NGSILD-Results-Count": "" }))).toBeUndefined();
    expect(parseResultsCount(new Headers({ "NGSILD-Results-Count": "many" }))).toBeUndefined();
    expect(parseResultsCount(new Headers({ "NGSILD-Results-Count": "-1" }))).toBeUndefined();
  });
});

describe("space helpers", () => {
  it("prefers the endpoint without a policy, then the first public one", () => {
    const narrowed = manifest("Endpoint", "a", { audience: "public", policyRef: "urn:x" });
    const open = manifest("Endpoint", "b", { audience: "organization" });
    const secondPublic = manifest("Endpoint", "c", { audience: "public", policyRef: "urn:y" });
    expect(pickReadEndpoint([narrowed, open])?.metadata.name).toBe("b");
    expect(pickReadEndpoint([narrowed, secondPublic])?.metadata.name).toBe("a");
    expect(pickReadEndpoint([manifest("Endpoint", "d", { audience: "organization", policyRef: "urn:z" })])).toBeUndefined();
    expect(pickReadEndpoint([])).toBeUndefined();
  });

  it("reads through the space's own endpoint before an App's (T-5604)", () => {
    const app = {
      ...manifest("Endpoint", "app-air-quality", { audience: "project-list", callerRole: true }),
      metadata: { name: "app-air-quality", namespace: "banskabystrica", annotations: { "joinedcontext.com/generated-by": "portal/app-reconciler" } },
    } as Manifest;
    const own = manifest("Endpoint", "air-quality", { audience: "public", policyRef: "urn:x" });
    expect(pickReadEndpoint([app, own], ["banskabystrica"], "banskabystrica")?.metadata.name).toBe("air-quality");
    expect(pickReadEndpoint([app, own])?.metadata.name).toBe("air-quality");
    // An App's endpoint is still read when the space offers the person nothing else.
    expect(pickReadEndpoint([app], ["banskabystrica"], "banskabystrica")?.metadata.name).toBe("app-air-quality");
  });

  it("names the space from a string ref, an object ref or the space label", () => {
    expect(spaceOf(manifest("Policy", "p", { contextSpaceRef: "ovzdusie" }))).toBe("ovzdusie");
    expect(
      spaceOf(manifest("Policy", "p", { contextSpaceRef: { kind: "ContextSpace", name: "doprava" } })),
    ).toBe("doprava");
    const labelled = manifest("Endpoint", "e", {});
    labelled.metadata.labels = { "joinedcontext.com/space": "voda" };
    expect(spaceOf(labelled)).toBe("voda");
    expect(spaceOf(manifest("Policy", "p", {}))).toBeUndefined();
  });

  it("reads entity types from spec.classes only; an inline LinkML source is not parsed (DM-56)", () => {
    expect(entityTypesOf(manifest("DataModel", "m", { classes: ["A", "B"] }))).toEqual(["A", "B"]);
    expect(
      entityTypesOf(
        manifest("DataModel", "m", {
          linkml: "id: https://x/air\nname: air\nclasses:\n  AirQualityObserved:\n    slots: []\n",
        }),
      ),
    ).toEqual([]);
    expect(entityTypesOf(manifest("DataModel", "m", { linkml: "./air.linkml.yaml" }))).toEqual([]);
  });
});

describe("space inside view", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/spaces/ovzdusie");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows each entity type with its live count and sample entities read through the gateway", async () => {
    const fetchMock = renderInside({ status: 200, count: 42 });

    expect(await screen.findByRole("heading", { name: "Air quality" })).toBeInTheDocument();
    await openTab(en.spaces.inside.model);
    const table = await screen.findByRole("table", { name: en.spaces.inside.types });
    const row = within(table).getByText("AirQualityObserved").closest("tr") as HTMLElement;
    expect(await within(row).findByText("42")).toBeInTheDocument();
    // T-2760: the entity's own name with the URN in its title and a copy button, and values a
    // person reads — not `location={"coordinates":…}` or `name={"languageMap":…}`.
    const first = within(row).getByTitle(SAMPLES[0].id);
    expect(first).toHaveTextContent(/^1$/);
    expect(within(row).getByTitle(SAMPLES[1].id)).toHaveTextContent(/^2$/);
    expect(
      within(row).getByRole("button", { name: en.spaces.inside.copyId.replace("{id}", SAMPLES[0].id) }),
    ).toBeInTheDocument();
    expect(within(row).getByText("pm10: 12 · name: SNP Square · location: Point [19.1461, 48.7356]")).toBeInTheDocument();
    expect(row.textContent).not.toMatch(/languageMap|coordinates|\{/);
    // T-6145: each sample's own size, its JSON as the gateway answered it, read aloud as a size.
    const bytes = new TextEncoder().encode(JSON.stringify(SAMPLES[0])).length;
    const sampleSize = within(row).getByTitle(i18n.t("spaces.inside.sampleSize", { bytes: `${bytes} B` }));
    expect(sampleSize).toHaveTextContent(`${en.spaces.inside.size}: ${bytes} B`);

    const gatewayCalls = fetchMock.mock.calls
      .map((call) => urlOf(call[0]))
      .filter((url) => url.pathname.startsWith("/api/endpoint/"));
    // One read per type: the count and the samples come in the same answer (F-R2UI4-014).
    const typeReads = gatewayCalls.filter((url) => url.searchParams.get("type") === "AirQualityObserved");
    expect(typeReads.map((url) => [url.searchParams.get("count"), url.searchParams.get("limit"), url.searchParams.get("options")])).toEqual([
      ["true", "3", "keyValues"],
    ]);
    expect(gatewayCalls.every((url) => url.origin === window.location.origin)).toBe(true);
  });

  // T-5598: the probe of the space surface no longer stands in front of the grid's own read, one
  // round trip on a phone's network; a refusal still replaces the grid (the tests below).
  it("reads and draws the rows while the probe of the space surface is still on its way", async () => {
    const fetchMock = renderInside({ status: 200, count: 42 }, { status: 200, holdProbe: true });
    expect(await screen.findByRole("button", { name: /^Open: / })).toBeInTheDocument();
    const surfaceReads = fetchMock.mock.calls
      .map((call) => urlOf(call[0]))
      .filter((url) => url.pathname.startsWith("/cs/ovzdusie/ngsi-ld/v1/entities"));
    expect(surfaceReads.map((url) => url.searchParams.get("limit"))).toEqual(expect.arrayContaining(["1", "100"]));
  });

  // T-2889: the whole space's count, the broker's, under the per-type breakdown on the model tab (T-4885).
  it("says how many entities the whole space holds, and why when it cannot", async () => {
    renderInside({ status: 200, count: 42 }, { status: 200 }, {
      usage: { status: 200, body: { entities: 14232, broker: "canary", observedAt: "2026-09-25T11:40:00Z" } },
    });
    await openTab(en.spaces.inside.model);
    expect(await screen.findByTestId("space-total")).toHaveTextContent("The space holds 14,232 entities.");
    // PF-116: the space's page names the broker it lives on.
    expect(screen.getByTestId("space-broker")).toHaveTextContent("It lives on the context broker canary.");
  });

  // T-2890: the size the broker measured, whole and per type, in binary units; none without it.
  it("says what the space takes in the store and each type's share, and no size when unmeasured", async () => {
    renderInside({ status: 200, count: 42 }, { status: 200 }, {
      usage: {
        status: 200,
        body: {
          entities: 42,
          broker: "main",
          observedAt: "2026-09-25T11:40:00Z",
          bytes: {
            entities: 3 * 1024 * 1024,
            attrInstances: 1024 * 1024,
            byType: { "https://smartdatamodels.org/dataModel.Environment/AirQualityObserved": 3 * 1024 * 1024 },
          },
        },
      },
    });
    await openTab(en.spaces.inside.model);
    const size = await screen.findByTestId("space-size");
    expect(size).toHaveTextContent(
      i18n.t("spaces.inside.totalSize", { size: "4 MiB", entities: "3 MiB", history: "1 MiB" }),
    );
    expect(size).toHaveAttribute("title", "4,194,304 B");
    const table = await screen.findByRole("table", { name: en.spaces.inside.types });
    const row = within(table).getByText("AirQualityObserved").closest("tr") as HTMLElement;
    expect(within(row).getByTitle("3,145,728 B")).toHaveTextContent("3 MiB");
    // what the type size counts against what a sample's size counts, in words (T-6156)
    expect(screen.getByText(en.spaces.inside.sizeHint)).toBeInTheDocument();
  });

  // T-6085: the history's count beside the entities', and the broker's word when it estimated it.
  it("says how many history records the space holds and when that is an estimate", async () => {
    renderInside({ status: 200, count: 42 }, { status: 200 }, {
      usage: {
        status: 200,
        body: {
          entities: 241000,
          attrInstances: 3250000,
          estimated: true,
          broker: "main",
          observedAt: "2026-10-29T18:40:00Z",
          bytes: { entities: 512 * 1024 * 1024, attrInstances: 1536 * 1024 * 1024, byType: {} },
        },
      },
    });
    await openTab(en.spaces.inside.model);
    expect(await screen.findByTestId("space-history")).toHaveTextContent("Its history holds 3,250,000 records.");
    expect(screen.getByTestId("space-size")).toHaveTextContent(
      "It takes 2 GiB in the broker's store: 512 MiB current state, 1.5 GiB history.",
    );
    expect(screen.getByTestId("space-estimated")).toHaveTextContent(en.spaces.inside.estimated);
  });

  it("claims no history count and no estimate the broker did not give", async () => {
    renderInside({ status: 200, count: 42 }, { status: 200 }, {
      usage: { status: 200, body: { entities: 42, estimated: false, broker: "main", observedAt: "2026-10-29T18:40:00Z" } },
    });
    await openTab(en.spaces.inside.model);
    expect(await screen.findByTestId("space-total")).toBeInTheDocument();
    expect(screen.queryByTestId("space-history")).toBeNull();
    expect(screen.queryByTestId("space-estimated")).toBeNull();
  });

  it("shows no size, never a zero, when the broker did not measure one", async () => {
    renderInside({ status: 200, count: 42 }, { status: 200 }, {
      usage: { status: 200, body: { entities: 42, broker: "main", observedAt: "2026-09-25T11:40:00Z" } },
    });
    await openTab(en.spaces.inside.model);
    expect(await screen.findByTestId("space-total")).toBeInTheDocument();
    expect(screen.queryByTestId("space-size")).toBeNull();
    const table = await screen.findByRole("table", { name: en.spaces.inside.types });
    const row = within(table).getByText("AirQualityObserved").closest("tr") as HTMLElement;
    // the type's size cell: the samples beside it carry their own JSON size (T-6145)
    const sizeCell = row.querySelectorAll("td")[2];
    expect(sizeCell).toHaveTextContent("—");
    expect(sizeCell.textContent).not.toMatch(/\d B|KiB|MiB/);
  });

  it("names the reason instead of a count the broker could not give", async () => {
    renderInside({ status: 200, count: 42 }, { status: 200 }, {
      usage: { status: 503, body: { title: "Service Unavailable", status: 503, detail: "the broker answered 500" } },
    });
    await openTab(en.spaces.inside.model);
    expect(await screen.findByTestId("space-total")).toHaveTextContent(
      i18n.t("spaces.entitiesUnknown", { reason: "the broker answered 500" }),
    );
  });

  it("takes the space's types from the model that names it, with no dataModelRef on the space", async () => {
    // How every seeded space is written: `DataModel.spec.contextSpaceRef` is required and names
    // the space, while the space's own `dataModelRef` is an optional pointer at the primary
    // model and no seed sets it. Reading only the pointer left this table empty on dev for
    // every space there is, each of them holding hundreds of entities (T-2450).
    const { dataModelRef: _pointer, ...spec } = SPACE.spec as Record<string, unknown>;
    renderInside(
      { status: 200, count: 532 },
      { status: 200 },
      {
        space: { ...SPACE, spec },
        models: list([
          manifest("DataModel", "statistical-observation", {
            contextSpaceRef: "ovzdusie",
            linkml: "./statistical-observation.linkml.yaml",
            version: "1.1.0",
            classes: ["StatisticalObservation"],
          }),
        ]),
      },
    );

    await openTab(en.spaces.inside.model);
    const table = await screen.findByRole("table", { name: en.spaces.inside.types });
    const row = within(table).getByText("StatisticalObservation").closest("tr") as HTMLElement;
    expect(await within(row).findByText("532")).toBeInTheDocument();
  });

  it("puts the model the space points at first, so the grid opens on it", async () => {
    renderInside(
      { status: 200, count: 1 },
      { status: 200 },
      {
        models: list([
          manifest("DataModel", "other", {
            contextSpaceRef: "ovzdusie",
            classes: ["Something"],
          }),
          manifest("DataModel", "bb-air-quality", {
            contextSpaceRef: "ovzdusie",
            classes: ["AirQualityObserved"],
          }),
        ]),
      },
    );

    await openTab(en.spaces.inside.model);
    const table = await screen.findByRole("table", { name: en.spaces.inside.types });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("AirQualityObserved");
    expect(rows.map((row) => row.textContent)).toHaveLength(2);
  });

  it("says an endpoint that publishes to no catalogue is not published, and links nowhere", async () => {
    const unpublished = {
      ...manifest("Endpoint", "public-air", {
        contextSpaceRef: "ovzdusie",
        slug: SLUG,
        audience: "public",
        enabledRepresentations: ["ngsi-ld"],
      }),
      status: { phase: "Live" },
    };
    renderInside({ status: 200, count: 1 }, { status: 200 }, { endpoints: list([unpublished]) });

    await openTab(en.endpoints.title);
    const endpointRow = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    expect(within(endpointRow).getByText(en.spaces.inside.notPublished)).toBeInTheDocument();
    expect(
      within(endpointRow).queryByRole("link", { name: en.spaces.inside.catalogueLink }),
    ).not.toBeInTheDocument();
  });

  it("lists only the endpoints and policies of this space, with a catalogue link per published endpoint", async () => {
    renderInside({ status: 200, count: 1 });

    await openTab(en.endpoints.title);
    const endpointRow = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    expect(within(endpointRow).getByText(en.endpoints.audience.public)).toBeInTheDocument();
    // the endpoint's name opens its own page
    expect(within(endpointRow).getByRole("link", { name: "public-air" })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/endpoints/public-air",
    );
    expect(within(endpointRow).getByRole("link", { name: "ngsi-ld" })).toHaveAttribute(
      "href",
      `${window.location.origin}/api/endpoint/${SLUG}/ngsi-ld/v1/types`,
    );
    // The dataset on its CkanInstance's own site (T-3018).
    expect(
      await within(endpointRow).findByRole("link", { name: en.spaces.inside.catalogueLink }),
    ).toHaveAttribute("href", "https://data.city.example/dataset/public-air");
    expect(screen.queryByText("other-space")).not.toBeInTheDocument();

    await openTab(en.spaces.inside.policies);
    const policyRow = (await screen.findByText("public-air-quality")).closest("tr") as HTMLElement;
    // the assignee in words and the operations as kinds, the CIM 009 names one hover away
    expect(within(policyRow).getByText(en.spaces.inside.policyPublic)).toHaveAttribute("title", "role:public");
    expect(within(policyRow).getByText(en.spaces.inside.operationKind.read)).toHaveAttribute(
      "title",
      "queryEntity, retrieveEntity",
    );
  });

  it("says a type's read met the endpoint's rate limit, with a retry, and not that it is unreadable (T-3161)", async () => {
    const fetchMock = renderInside({ status: 429 });
    await openTab(en.spaces.inside.model);
    const table = await screen.findByRole("table", { name: en.spaces.inside.types });
    const row = within(table).getByText("AirQualityObserved").closest("tr") as HTMLElement;
    expect(await within(row).findByText(en.spaces.inside.tooMany)).toBeInTheDocument();
    const before = fetchMock.mock.calls.length;
    await userEvent.click(within(row).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() => expect(fetchMock.mock.calls.length).toBeGreaterThan(before));
  });

  it("asks one endpoint one question at a time, so its rate limit is not met at once (T-3161)", async () => {
    let open = 0;
    let most = 0;
    const read = () =>
      inTurn("slug-a", async () => {
        open += 1;
        most = Math.max(most, open);
        await new Promise((resolve) => setTimeout(resolve, 5));
        open -= 1;
      });
    await Promise.all([read(), read(), read(), inTurn("slug-a", () => Promise.reject(new Error("x"))).catch(() => undefined), read()]);
    expect(most).toBe(1);
  });

  it("tells a signed-in person refused by the gateway that they hold no read grant (T-6156)", async () => {
    renderInside({ status: 403 }, { status: 200 }, {
      usage: { status: 403, body: { title: "Forbidden", status: 403, detail: "you may not read this space's usage" } },
    });

    await openTab(en.spaces.inside.model);
    const table = await screen.findByRole("table", { name: en.spaces.inside.types });
    const row = within(table).getByText("AirQualityObserved").closest("tr") as HTMLElement;
    // one sentence over the count and size cells: no count, no size, no blank cell, no dash
    const refusal = await within(row).findByText(en.spaces.inside.noReadGrant);
    expect(refusal.closest("td")).toHaveAttribute("colspan", "2");
    expect(within(row).queryByText(en.spaces.inside.notReadable)).toBeNull();
    expect(row.querySelectorAll("td")).toHaveLength(2);
    expect(row.textContent).not.toMatch(/—|\d B|KiB|MiB/);
  });

  it("says anonymous for a caller with no session, a missing grant for a signed-in one", () => {
    expect(refusalKey(false)).toBe("spaces.inside.notReadable");
    expect(refusalKey(true)).toBe("spaces.inside.noReadGrant");
  });

  it("stacks the samples under their type, so a phone that hides no column still shows them (T-6156)", async () => {
    renderInside({ status: 200, count: 42 });
    await openTab(en.spaces.inside.model);
    const table = await screen.findByRole("table", { name: en.spaces.inside.types });
    // three columns: type with its samples, entities, size; no samples column a phone drops
    expect(within(table).getAllByRole("columnheader").map((th) => th.textContent)).toEqual([
      en.spaces.inside.type,
      en.spaces.inside.count,
      en.spaces.inside.size,
    ]);
    const row = within(table).getByText("AirQualityObserved").closest("tr") as HTMLElement;
    const typeCell = row.querySelectorAll("td")[0];
    const samples = await within(typeCell).findByRole("list", { name: en.spaces.inside.samples });
    expect(within(samples).getByTitle(SAMPLES[0].id)).toBeInTheDocument();
    expect(typeCell.className).not.toMatch(/hidden/);
    // the type name ends in "…" with its whole name on hover, never broken mid-word
    const name = within(typeCell).getByTitle("AirQualityObserved");
    expect(name.className).toMatch(/truncate/);
    expect(name.className).not.toMatch(/overflow-wrap/);
    // no size measured, so no words on what a size counts
    expect(screen.queryByText(en.spaces.inside.sizeHint)).toBeNull();
  });
});

/**
 * T-1434, SP-04, SP-06: the space itself in the grid, not one endpoint's view of it. The surface
 * answers by the caller's own grants, and a caller with none gets a 404 that says nothing about
 * whether the space exists — which is where the page points at the endpoints instead.
 */
describe("the space's own data", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/spaces/ovzdusie");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("lists the model's types and reads the space surface, never an endpoint", async () => {
    const fetchMock = renderInside({ status: 200, count: 1 });

    // The grid's Type column lists the model's types (owner 2026-11-01, T-6344).
    expect(await screen.findByText("12 µg/m³")).toBeInTheDocument();
    const picker = screen.getByRole("combobox", { name: en.spaces.inside.type });
    expect(within(picker).getAllByRole("option").map((option) => (option as HTMLOptionElement).value)).toEqual([
      "AirQualityObserved",
      "AirQualityStation",
    ]);

    const reads = fetchMock.mock.calls
      .map((call) => urlOf(call[0]))
      .filter((url) => url.pathname.includes("/ngsi-ld/v1/entities"));
    const surface = reads.filter((url) => url.pathname.startsWith("/cs/ovzdusie/"));
    expect(surface).not.toHaveLength(0);
    expect(surface.every((url) => url.searchParams.get("type") === "AirQualityObserved")).toBe(true);
  });

  it("picks the type in the grid's Type column at every width, and picking one reads that type (T-4992, T-6344)", async () => {
    const fetchMock = renderInside({ status: 200, count: 1 });

    const picker = await screen.findByRole("combobox", { name: en.spaces.inside.type });
    expect(within(picker).getAllByRole("option").map((option) => (option as HTMLOptionElement).value)).toEqual([
      "AirQualityObserved",
      "AirQualityStation",
    ]);
    expect(picker).toHaveValue("AirQualityObserved");

    fetchMock.mockClear();
    await userEvent.selectOptions(picker, "AirQualityStation");
    expect(picker).toHaveValue("AirQualityStation");
    await waitFor(() =>
      expect(
        fetchMock.mock.calls
          .map((call) => urlOf(call[0]))
          .some((url) => url.pathname.startsWith("/cs/ovzdusie/") && url.searchParams.get("type") === "AirQualityStation"),
      ).toBe(true),
    );
    // No second list beside the grid repeats it: a space without saved views has no left list.
    expect(screen.queryByRole("navigation", { name: en.spaces.inside.types })).toBeNull();
  });

  it("offers the Grid and API tabs only (owner 2026-10-31, T-6210 deletes the other views)", async () => {
    renderInside({ status: 200, count: 1 }, { status: 200 });
    const tabs = await screen.findAllByRole("tab", { name: /^(Grid|Gallery|Board|Calendar|Timeline|Map|Form|API)$/ });
    expect(tabs.map((tab) => tab.textContent)).toEqual([en.spaces.views.kind.grid, en.spaces.views.kind.api]);
  });

  it("keeps the header on one row: the name, its purpose behind the info mark, no copy chip (owner 2026-10-31)", async () => {
    const description = "Air quality stations of the city and what they measured every hour, kept for five years.";
    renderInside({ status: 200, count: 1 }, { status: 200 }, {
      space: { ...SPACE, metadata: { ...SPACE.metadata, description: { en: description } } },
    });
    await screen.findByRole("heading", { level: 1 });
    expect(screen.queryByRole("button", { name: "ovzdusie" })).toBeNull();
    expect(screen.getByRole("tooltip", { hidden: true })).toHaveTextContent(description);
    // The walker still finds the purpose line, now behind the mark as on the list pages (T-6095).
    expect(pageFindings({ namespaces: [], layout: false }).filter((f) => f.startsWith("purpose-line"))).toEqual([]);
  });

  it("folds the grid's query behind the toolbar's Query toggle (T-5093)", async () => {
    renderInside({ status: 200, count: 1 });
    expect(await screen.findByText("12 µg/m³")).toBeInTheDocument();
    expect(screen.queryByLabelText(en.entityGrid.editAsText)).toBeNull();
    const toggle = screen.getByRole("button", { name: en.spaces.toolbar.query });
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(await screen.findByLabelText(en.entityGrid.editAsText)).toBeInTheDocument();
  });

  it("names a failed surface read with its status and a retry", async () => {
    renderInside({ status: 200, count: 1 }, { status: 502 });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(en.spaces.inside.dataFailed);
    expect(alert).toHaveTextContent("HTTP 502");
    expect(within(alert).getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expect(screen.queryByText(en.spaces.inside.dataThroughEndpoints)).toBeNull();
  });

  // T-5601: only a refusal lists the endpoints; a busy or unreachable surface is a retry.
  it.each([503, 504])("names a %s surface read with its status and a retry", async (status) => {
    renderInside({ status: 200, count: 1 }, { status });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(`HTTP ${status}`);
    expect(within(alert).getByRole("button", { name: en.app.error.retry })).toBeEnabled();
    expect(screen.queryByText(en.spaces.inside.dataThroughEndpoints)).toBeNull();
  });

  it("holds the retry for a 429's Retry-After and lists no endpoints", async () => {
    renderInside({ status: 200, count: 1 }, { status: 429, headers: { "Retry-After": "30" } });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(en.spaces.inside.dataFailed);
    expect(alert).toHaveTextContent("The Portal is busy right now; nothing was changed. Try again in 30 seconds.");
    expect(within(alert).getByRole("button", { name: en.app.error.retry })).toHaveAttribute("aria-disabled", "true");
    expect(screen.queryByText(en.spaces.inside.dataThroughEndpoints)).toBeNull();
    expect(screen.queryByRole("link", { name: "public-air" })).toBeNull();
  });

  it("frees the retry once a 429's Retry-After has passed", async () => {
    renderInside({ status: 200, count: 1 }, { status: 429, headers: { "Retry-After": "1" } });
    const retry = within(await screen.findByRole("alert")).getByRole("button", { name: en.app.error.retry });
    expect(retry).toHaveAttribute("aria-disabled", "true");
    await waitFor(() => expect(retry).not.toHaveAttribute("aria-disabled"), { timeout: 3000 });
  });

  it.each([429, 408, 0])("calls a %s probe a busy Portal with a retry", async (status) => {
    renderInside({ status: 200, count: 1 }, { status });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(en.spaces.inside.dataBusy);
    expect(within(alert).getByRole("button", { name: en.app.error.retry })).toBeEnabled();
    expect(screen.queryByText(en.spaces.inside.dataThroughEndpoints)).toBeNull();
  });

  it.each([401, 403])("points at the endpoints when the surface refuses with %s", async (status) => {
    renderInside({ status: 200, count: 1 }, { status });
    expect(await screen.findByText(en.spaces.inside.dataThroughEndpoints)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("points at the endpoints when the surface answers this person nothing", async () => {
    renderInside({ status: 200, count: 1 }, { status: 404 });

    expect(await screen.findByText(en.spaces.inside.dataThroughEndpoints)).toBeInTheDocument();
    // The endpoints of this space, and not the one of another space.
    const links = screen.getAllByRole("link", { name: "public-air" });
    expect(links[0]).toHaveAttribute("href", `${window.location.origin}/api/endpoint/${SLUG}`);
    expect(screen.queryByText(en.spaces.inside.dataEmpty)).toBeNull();
  });

  it("offers to add a field to a type its model declares, as a model change (T-3098)", async () => {
    renderInside({ status: 200, count: 1 }, { status: 200 }, {
      source: "name: bb-air-quality\nclasses:\n  AirQualityObserved:\n    attributes:\n      no2: { range: float }\n",
    });

    const add = await screen.findByRole("button", { name: en.spaces.fields.add });
    await userEvent.click(add);
    expect(await screen.findByRole("dialog", { name: en.spaces.fields.title.replace("{type}", "AirQualityObserved") })).toBeInTheDocument();
  });

  // T-4828: a row opens as a dialog named in the address, with its comments as a tab.
  it("opens the row a link names as a dialog with Fields, History, Metadata and Comments, and closing drops it from the address", async () => {
    const urn = SPACE_ROWS[0].id;
    window.history.pushState({}, "", `/projects/banskabystrica/spaces/ovzdusie?row=${encodeURIComponent(urn)}`);
    renderInside({ status: 200, count: 1 });
    const dialog = await screen.findByRole("dialog", { name: `${en.entityGrid.rowDetail}: ${urn.split(":").at(-1)}` });
    expect(within(dialog).getAllByRole("tab").map((tab) => tab.textContent)).toEqual([
      en.entityGrid.rowFields,
      en.entityGrid.history,
      en.entityGrid.metadata,
      // The space surface answers the Changes read, so the tab shows while it loads (T-6333).
      en.entityGrid.changes.tab,
      en.spaces.comments.title,
    ]);
    await userEvent.click(within(dialog).getByRole("button", { name: en.entityGrid.close }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(new URLSearchParams(window.location.search).get("row")).toBeNull();
  });

  it("puts the row opened from the grid into the address, and ignores a row parameter that is no URN", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/spaces/ovzdusie?row=javascript%3Aalert(1)");
    renderInside({ status: 200, count: 1 });
    const urn = SPACE_ROWS[0].id;
    await userEvent.click(await screen.findByRole("button", { name: `${en.entityGrid.openRow}: ${urn.split(":").at(-1)}` }));
    expect(await screen.findByRole("dialog", { name: `${en.entityGrid.rowDetail}: ${urn.split(":").at(-1)}` })).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("row")).toBe(urn);
  });

  it("opens the type's form from Add row and creates through the space surface", async () => {
    const fetchMock = renderInside({ status: 200, count: 1 }, { status: 200 }, {
      source: "name: bb-air-quality\nclasses:\n  AirQualityObserved:\n    attributes:\n      no2: { range: float }\n",
    });
    await userEvent.click(await screen.findByRole("button", { name: en.spaces.inside.addRow }));
    const dialog = await screen.findByRole("dialog", { name: en.spaces.inside.addRowTitle.replace("{type}", "AirQualityObserved") });
    expect(within(dialog).getByRole("button", { name: en.spaces.form.submit })).toBeInTheDocument();
    expect(fetchMock.mock.calls.some((call) => urlOf(call[0]).pathname.includes("/temporal/"))).toBe(false);
  });

  it("offers no entity write inside a copy, whose rows are the live space, and points at Try it (T-4868)", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/spaces/ovzdusie?workspace=air-v2");
    const fetchMock = renderInside({ status: 200, count: 1 }, { status: 200 }, {
      source: "name: bb-air-quality\nclasses:\n  AirQualityObserved:\n    attributes:\n      no2: { range: float }\n",
    });
    const note = (await screen.findByText(en.spaces.inside.copyLive)).closest("[role=status]") as HTMLElement;
    expect(within(note).getByRole("link", { name: en.workspaces.bar.tryIt })).toHaveAttribute(
      "href",
      expect.stringMatching(/^\/projects\/banskabystrica\/workspaces\/air-v2\/try-it(\?|$)/),
    );
    expect(await screen.findByRole("button", { name: en.spaces.inside.addRow })).toHaveAttribute("aria-disabled", "true");
    expect(await screen.findByText("12 µg/m³")).toBeInTheDocument();
    const writes = fetchMock.mock.calls.filter((call) => (call[1] as RequestInit | undefined)?.method && (call[1] as RequestInit).method !== "GET");
    expect(writes.filter((call) => urlOf(call[0]).pathname.includes("/ngsi-ld/"))).toHaveLength(0);
  });

  it("offers no field to add while the model's source does not declare the type", async () => {
    renderInside({ status: 200, count: 1 });

    expect(await screen.findByText("12 µg/m³")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.spaces.fields.add })).toBeNull();
  });

  it("applies a saved view: its query is what the space surface is asked (T-3104)", async () => {
    const view = {
      id: "7f1c2a9e-4b1d-4a57-9a0e-2f6d1c3b8e01",
      type: "AirQualityObserved",
      kind: "grid",
      mode: "collaborative",
      title: "High NO2",
      owner: "jana.kovacova",
      config: { q: "no2>40" },
      version: 1,
      createdAt: "2026-10-06T19:00:00Z",
      updatedAt: "2026-10-06T19:00:00Z",
    };
    const fetchMock = renderInside({ status: 200, count: 1 }, { status: 200 }, { views: [view] });

    const picker = await screen.findByLabelText(en.spaces.saved.view);
    await waitFor(() => expect(within(picker).getAllByRole("option")).toHaveLength(2));
    await userEvent.selectOptions(picker, view.id);

    await waitFor(() => {
      const asked = fetchMock.mock.calls
        .map((call) => urlOf(call[0]))
        .filter((url) => url.pathname.startsWith("/cs/ovzdusie/ngsi-ld/v1/entities"));
      expect(asked.some((url) => url.searchParams.get("q") === "no2>40")).toBe(true);
    });
  });

  it("opens a saved view of a kind the page no longer has as the grid, with its query (T-6210)", async () => {
    const view = {
      id: "7f1c2a9e-4b1d-4a57-9a0e-2f6d1c3b8e09",
      type: "AirQualityObserved",
      kind: "kanban",
      mode: "collaborative",
      title: "Old board",
      owner: "jana.kovacova",
      config: { q: "no2>40" },
      version: 1,
      createdAt: "2026-10-06T19:00:00Z",
      updatedAt: "2026-10-06T19:00:00Z",
    };
    const fetchMock = renderInside({ status: 200, count: 1 }, { status: 200 }, { views: [view] });
    // Only the grid and the API are offered.
    const tabs = within(await screen.findByRole("tablist", { name: en.spaces.views.label })).getAllByRole("tab");
    expect(tabs.map((tab) => tab.textContent)).toEqual([en.spaces.views.kind.grid, en.spaces.views.kind.api]);

    const picker = await screen.findByLabelText(en.spaces.saved.view);
    await waitFor(() => expect(within(picker).getAllByRole("option")).toHaveLength(2));
    await userEvent.selectOptions(picker, view.id);

    await waitFor(() => {
      const asked = fetchMock.mock.calls
        .map((call) => urlOf(call[0]))
        .filter((url) => url.pathname.startsWith("/cs/ovzdusie/ngsi-ld/v1/entities"));
      expect(asked.some((url) => url.searchParams.get("q") === "no2>40")).toBe(true);
    });
    expect(screen.getByRole("tab", { name: en.spaces.views.kind.grid })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("grid")).toBeInTheDocument();
  });

  it("marks the rows a view's colour rule matches, and hides the fields it hides (T-3099)", async () => {
    const rows = [
      { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:1", type: "AirQualityObserved", pm10: { type: "Property", value: 52 }, no2: { type: "Property", value: 9 } },
      { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:2", type: "AirQualityObserved", pm10: { type: "Property", value: 12 }, no2: { type: "Property", value: 7 } },
    ];
    const view = {
      id: "7f1c2a9e-4b1d-4a57-9a0e-2f6d1c3b8e02",
      type: "AirQualityObserved",
      kind: "grid",
      mode: "personal",
      title: "Dusty",
      owner: "jana.kovacova",
      config: { hidden: ["no2"], colour: [{ when: "pm10>50", colour: "danger" }] },
      version: 1,
      createdAt: "2026-10-06T19:00:00Z",
      updatedAt: "2026-10-06T19:00:00Z",
    };
    renderInside({ status: 200, count: 1 }, { status: 200, rows }, { views: [view] });

    const picker = await screen.findByLabelText(en.spaces.saved.view);
    await waitFor(() => expect(within(picker).getAllByRole("option")).toHaveLength(2));
    expect(await screen.findByRole("columnheader", { name: /NO₂/ })).toBeInTheDocument();
    await userEvent.selectOptions(picker, view.id);

    const marked = await screen.findAllByRole("img", { name: en.spaces.saved.toneLabel.replace("{when}", "pm10>50") });
    expect(marked).toHaveLength(1);
    expect(marked[0].closest("tr")).toHaveTextContent("52");
    expect(screen.queryByRole("columnheader", { name: /NO₂/ })).toBeNull();
  });

  // T-4827: values by what they hold, the model's order and labels, on the caller's own answer.
  const KPI_MODEL = [
    "name: bb-air-quality",
    "classes:",
    "  AirQualityObserved:",
    "    attributes:",
    "      secretNote: { range: string }",
    "      refStation: { range: string, annotations: { ngsi_ld_kind: Relationship } }",
    "      calculationPeriod: { range: string }",
    "      pm10: { range: float, annotations: { label: { en: Particulates } } }",
    "      name: { range: string, rank: 1 }",
    "",
  ].join("\n");
  const KPI_ROWS = [
    {
      id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:1",
      type: "AirQualityObserved",
      name: { type: "Property", value: "Námestie SNP" },
      pm10: { type: "Property", value: 12.5, unitCode: "GQ" },
      calculationPeriod: { type: "Property", value: { start: "2026-10-01", end: "2026-10-31" } },
      refStation: { type: "Relationship", object: "urn:ngsi-ld:Station:banskabystrica.sk:ovzdusie:7" },
      "ngsi-ld:status": { type: "Property", value: "ok" },
    },
  ];
  const headerNames = () =>
    screen.getAllByRole("columnheader").map((th) => within(th).queryAllByRole("button")[0]?.textContent ?? "").filter(Boolean);

  it("shows no raw JSON, URN or IRI, keeps the model's order and labels, and only what the answer carries", async () => {
    renderInside({ status: 200, count: 1 }, { status: 200, rows: KPI_ROWS }, { source: KPI_MODEL });

    expect(await screen.findByRole("columnheader", { name: /Particulates/ })).toBeInTheDocument();
    // T-6210: id, type and name lead, then the model's order.
    await waitFor(() => expect(headerNames().slice(0, 7)).toEqual(["ID", "Type", "Name", "Ref station", "Calculation period", "Particulates (µg/m³)", "Status"]));
    // DM-102: the model declares secretNote, the caller's answer does not carry it.
    expect(screen.queryByRole("columnheader", { name: /secret/i })).toBeNull();
    const grid = screen.getByRole("grid");
    const cells = within(grid).getAllByRole("gridcell").map((cell) => cell.textContent ?? "");
    expect(cells.join(" ")).not.toMatch(/[{}]|urn:|ngsi-ld:/);
    expect(cells.join(" ").replace(/\u00a0/g, " ").replace(/\u2060/g, "")).toContain("Oct 1, 2026 – Oct 31, 2026");
    expect(cells.join(" ")).toContain("12.5 µg/m³");
    // The stub answers no Station, so the target reads as its id's end, with no link (G-R2GRID2-020).
    const target = await within(grid).findByTitle("urn:ngsi-ld:Station:banskabystrica.sk:ovzdusie:7");
    expect(target.tagName).toBe("SPAN");
    expect(target).toHaveTextContent("7");
  });

  it("opens with the layout kept in this browser, even though the type arrives after the first render", async () => {
    window.localStorage.clear();
    writeLayout("banskabystrica", "ovzdusie", "AirQualityObserved", { labels: { "ngsi-ld:status": "State" }, hidden: ["calculationPeriod"] });
    renderInside({ status: 200, count: 1 }, { status: 200, rows: KPI_ROWS }, { source: KPI_MODEL });
    expect(await screen.findByRole("button", { name: `${en.entityGrid.columnMenu}: State` })).toBeInTheDocument();
    expect(screen.queryByRole("columnheader", { name: /Calculation period/ })).toBeNull();
    // Opening wrote nothing over it.
    expect(readLayout("banskabystrica", "ovzdusie", "AirQualityObserved")).toMatchObject({ labels: { "ngsi-ld:status": "State" } });
  });

  it("keeps a renamed, moved and hidden column across a reload, and saves them with a view", async () => {
    window.localStorage.clear();
    const fetchMock = renderInside({ status: 200, count: 1 }, { status: 200, rows: KPI_ROWS }, { source: KPI_MODEL });
    await screen.findByRole("columnheader", { name: /Particulates/ });
    await userEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: Status` }));
    const field = screen.getByLabelText(en.entityGrid.renameColumn);
    await userEvent.clear(field);
    await userEvent.type(field, "State{Enter}");
    await userEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: State` }));
    await userEvent.click(screen.getByRole("button", { name: en.entityGrid.hideColumn }));
    await userEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: Ref station` }));
    await userEvent.click(screen.getByRole("button", { name: en.entityGrid.moveRight }));
    await waitFor(() => expect(headerNames().slice(0, 5)).toEqual(["ID", "Type", "Name", "Calculation period", "Ref station"]));
    expect(screen.queryByRole("columnheader", { name: /State/ })).toBeNull();

    // "All rows" keeps them in this browser: a reload reads them back (viewLayout.readLayout).
    const keptKey = Object.keys(window.localStorage).find((key) => key.endsWith(".ovzdusie.AirQualityObserved") && key.startsWith("jc.space-columns."));
    expect(keptKey).toBeDefined();
    const [, , project] = keptKey!.split(".");
    expect(readLayout(project, "ovzdusie", "AirQualityObserved")).toMatchObject({ labels: { "ngsi-ld:status": "State" }, hidden: ["ngsi-ld:status"] });

    await userEvent.click(screen.getByRole("button", { name: en.spaces.saved.saveNew }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByRole("textbox", { name: new RegExp(en.spaces.saved.title) }), "Mine");
    await userEvent.click(within(dialog).getByRole("button", { name: en.spaces.saved.confirm }));
    const posted = await waitFor(() => {
      const call = fetchMock.mock.calls.find(
        ([input]) => input instanceof Request && input.method === "POST" && urlOf(input).pathname.endsWith("/spaces/ovzdusie/views"),
      );
      expect(call).toBeDefined();
      return call![0] as Request;
    });
    const body = (await posted.clone().json()) as { config: Record<string, unknown> };
    expect(body.config.labels).toEqual({ "ngsi-ld:status": "State" });
    expect(body.config.hidden).toEqual(["ngsi-ld:status"]);
    expect((body.config.order as string[]).indexOf("refStation")).toBeGreaterThan((body.config.order as string[]).indexOf("calculationPeriod"));
  });

  // T-4264, T-4160 #6, T-6210: the grid leads with the id, the type and the entity's name (owner
  // 2026-10-31), the id cell opens the row by the name with the URN as its tooltip, and the headers
  // and the alerts button say what the model titles them.
  it("leads the data with id, type and the entity's name, and the model's titles for the keys", async () => {
    const rows = [
      {
        id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:1",
        type: "AirQualityObserved",
        name: { type: "Property", value: "Námestie SNP" },
        pm10: { type: "Property", value: 12 },
      },
    ];
    const source = [
      "id: https://banskabystrica.sk/models/air",
      "name: bb-air-quality",
      "classes:",
      "  AirQualityObserved:",
      "    title: { en: Air quality reading }",
      "    slots: [name, pm10]",
      "slots:",
      "  name: { range: string, title: { en: Station } }",
      "  pm10: { range: float, title: { en: Particles PM10 } }",
      "",
    ].join("\n");
    renderInside({ status: 200, count: 1 }, { status: 200, rows }, { source });

    const station = await screen.findByRole("columnheader", { name: /Station/ });
    // After the row-number column with its select-all checkbox (T-4883): id, type, then the name.
    const row = station.closest("tr") as HTMLTableRowElement;
    const headers = within(row).getAllByRole("columnheader");
    expect(within(headers[0]!).getByRole("checkbox", { name: "Select all loaded rows" })).toBeInTheDocument();
    expect(headers[1]).toHaveTextContent(/^ID/);
    expect(headers[2]).toHaveTextContent(/^Type/);
    expect(headers[3]).toBe(station);
    expect(within(row).getByRole("columnheader", { name: /Particles PM10/ })).toBeInTheDocument();
    expect(within(row).queryByRole("columnheader", { name: /^pm10$/ })).toBeNull();
    const open = screen.getByRole("button", { name: `${en.entityGrid.openRow}: Námestie SNP` });
    expect(open).toHaveAttribute("title", rows[0].id);
    expect(screen.getByRole("button", { name: en.alerts.forType.replace("{type}", "Air quality reading") })).toBeInTheDocument();
  });

  // T-4826: the grid edits only where the gateway's grant names a write on the type, and the write
  // goes through that endpoint with the person's reason; a viewer gets no editor at all.
  describe("editing in the grid", () => {
    const rows = [
      {
        id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:1",
        type: "AirQualityObserved",
        name: { type: "Property", value: "Námestie SNP" },
        pm10: { type: "Property", value: 12, unitCode: "GQ" },
      },
    ];
    const source = [
      "id: https://banskabystrica.sk/models/air",
      "name: bb-air-quality",
      "classes:",
      "  AirQualityObserved:",
      "    slots: [name, pm10]",
      "slots:",
      "  name: { range: string }",
      "  pm10: { range: float }",
      "",
    ].join("\n");
    const steward = {
      permissions: [
        { resource: { type: "AirQualityObserved" }, actions: ["queryEntity"], attributes: "*" },
        { resource: { type: "AirQualityObserved" }, actions: ["updateAttrs"], attributes: ["pm10"] },
      ],
    };
    /** The same grant as the gateway decides it per attribute (T-5528). */
    const stewardWrites = {
      type: "AirQualityObserved",
      attributes: [
        { name: "name", read: true, write: false, delete: false, reason: "not_in_grant" },
        { name: "pm10", read: true, write: true, delete: false, reason: "grant" },
      ],
    };

    it("offers an editor for the attributes the grant lets the steward write, and sends the reason", async () => {
      const fetchMock = renderInside({ status: 200, count: 1 }, { status: 200, rows }, { source, access: steward, attributes: stewardWrites });
      await userEvent.click(await screen.findByRole("button", { name: en.spaces.toolbar.edit }));
      // The grid names the column "PM10 (µg/m³)" once the rows are in (T-5598: it no longer waits for the probe).
      const cell = await screen.findByRole("spinbutton", { name: /pm10/i });
      // The grant names pm10 only: the name stays text, with no editor.
      expect(screen.queryByDisplayValue("Námestie SNP")).toBeNull();
      await userEvent.clear(cell);
      await userEvent.type(cell, "14");
      await userEvent.click(screen.getByRole("button", { name: en.entityGrid.review }));
      await userEvent.click(screen.getByRole("button", { name: en.entityGrid.apply }));

      await waitFor(() => expect(fetchMock.mock.calls.some(([, init]) => init?.method === "PATCH")).toBe(true));
      const [path, init] = fetchMock.mock.calls.find(([, each]) => each?.method === "PATCH")!;
      // Written through the endpoint that granted it, never the read-only space surface.
      expect(String(path)).toBe(`/api/endpoint/${SLUG}/ngsi-ld/v1/entities/${encodeURIComponent(rows[0].id)}/attrs`);
      expect((init!.headers as Record<string, string>)["JC-Reason"]).toBe(encodeURIComponent(en.entityGrid.reasonDefault));
      // A grid edit keeps the replaced value as a version for the row's Changes (T-6333).
      expect((init!.headers as Record<string, string>)["JC-Record"]).toBe("version");
      expect(JSON.parse(String(init!.body))).toEqual({ pm10: { type: "Property", value: 14, unitCode: "GQ" } });
      expect(await screen.findByRole("button", { name: en.entityGrid.undo })).toBeInTheDocument();
    });

    it("reads the grid's first page once, and the probe asks no count (T-6161)", async () => {
      const fetchMock = renderInside({ status: 200, count: 1 }, { status: 200, rows }, { source, access: steward, attributes: stewardWrites });
      await userEvent.click(await screen.findByRole("button", { name: en.spaces.toolbar.edit }));
      await screen.findByRole("spinbutton", { name: /pm10/i });
      const reads = fetchMock.mock.calls
        .map((call) => urlOf(call[0]))
        .filter((url) => url.pathname.startsWith("/cs/ovzdusie/ngsi-ld/v1/entities"));
      // The writer's grant lands after the first read: it changes where a correction goes, never the read.
      // The count is its own read with no rows (T-6541), asked once beside the page.
      const pages = reads.filter((url) => url.searchParams.get("limit") !== "1" && url.searchParams.get("limit") !== "0");
      expect(reads.filter((url) => url.searchParams.get("limit") === "0")).toHaveLength(1);
      expect(pages.map((url) => url.search)).toHaveLength(1);
      // The probe only asks whether the surface answers: counting the type there costs a second scan.
      const probes = reads.filter((url) => url.searchParams.get("limit") === "1");
      expect(probes.every((url) => url.searchParams.get("count") !== "true")).toBe(true);
    });

    // T-6209: Helsinki's Event tab sent 10 /access and 10 /access/attributes reads on every load.
    it("asks no endpoint for the person's access on load, only once Edit is turned on", async () => {
      const fetchMock = renderInside({ status: 200, count: 1 }, { status: 200, rows }, { source, access: steward, attributes: stewardWrites });
      const accessReads = () => fetchMock.mock.calls.filter(([input]) => /\/access(\/attributes)?$/.test(urlOf(input).pathname));
      await screen.findByRole("button", { name: `${en.entityGrid.openRow}: Námestie SNP` });
      expect(accessReads()).toEqual([]);
      expect(within(screen.getByRole("grid")).queryByRole("spinbutton")).toBeNull();

      const edit = screen.getByRole("button", { name: en.spaces.toolbar.edit });
      expect(edit).toHaveAttribute("aria-pressed", "false");
      await userEvent.click(edit);
      expect(edit).toHaveAttribute("aria-pressed", "true");
      await screen.findByRole("spinbutton", { name: /pm10/i });
      // One grant document and one decision table for the one endpoint of the space.
      expect(accessReads().map(([input]) => urlOf(input).pathname).sort()).toEqual([`/api/endpoint/${SLUG}/access`, `/api/endpoint/${SLUG}/access/attributes`]);

      await userEvent.click(edit);
      await waitFor(() => expect(within(screen.getByRole("grid")).queryByRole("spinbutton")).toBeNull());
    });

    it("gives a viewer, whose grant names no write, no editor", async () => {
      renderInside({ status: 200, count: 1 }, { status: 200, rows }, {
        source,
        access: { permissions: [{ resource: { type: "AirQualityObserved" }, actions: ["queryEntity"], attributes: "*" }] },
      });
      // The columns follow the answer (DM-102): the open button settles in the Name column once rows arrive.
      await screen.findAllByRole("button", { name: /: Name$/ });
      await userEvent.click(screen.getByRole("button", { name: en.spaces.toolbar.edit }));
      // The page says why Edit changed nothing, once the endpoint answered.
      expect(
        await screen.findByText(en.spaces.toolbar.editNone.replace("{type}", "AirQualityObserved")),
      ).toBeInTheDocument();
      expect(screen.getByRole("button", { name: `${en.entityGrid.openRow}: Námestie SNP` })).toBeInTheDocument();
      // In the grid: the toolbar's Share view panel holds an "Endpoint name" box of its own (T-5186).
      const grid = screen.getByRole("grid");
      expect(within(grid).queryByRole("spinbutton")).toBeNull();
      expect(within(grid).queryByRole("textbox", { name: /pm10|name/ })).toBeNull();
    });
  });

  // T-5753: the catalogue's draft model names no slot, and every endpoint answered its question with a 400.
  it("asks no endpoint for its decisions on a type whose model names no field", async () => {
    const source = ["id: https://banskabystrica.sk/models/air", "name: bb-air-quality", "classes:", "  AirQualityObserved:", "    description: a draft", ""].join("\n");
    const fetchMock = renderInside({ status: 200, count: 1 }, { status: 200 }, { source });
    await screen.findByRole("grid");
    await waitFor(() => expect(fetchMock.mock.calls.some(([input]) => urlOf(input).pathname.endsWith("/datamodels/bb-air-quality/source"))).toBe(true));
    expect(fetchMock.mock.calls.filter(([input]) => urlOf(input).pathname.endsWith("/access/attributes"))).toEqual([]);
  });

  // T-5769: Helsinki's catalogue model names slots but ships no JSON Schema, the gateway's only
  // source of served attributes, and every endpoint answered the question with a 400.
  it("asks no endpoint for its decisions when the model ships no JSON Schema", async () => {
    const source = ["id: https://banskabystrica.sk/models/air", "name: bb-air-quality", "classes:", "  AirQualityObserved:", "    slots: [pm10]", "slots:", "  pm10:", "    range: float", ""].join("\n");
    const models = list([manifest("DataModel", "bb-air-quality", { linkml: "./bb-air-quality.linkml.yaml", version: "0.1.0", classes: ["AirQualityObserved"] })]);
    const fetchMock = renderInside({ status: 200, count: 1 }, { status: 200 }, { source, models });
    await screen.findByRole("grid");
    await waitFor(() => expect(fetchMock.mock.calls.some(([input]) => urlOf(input).pathname.endsWith("/datamodels/bb-air-quality/source"))).toBe(true));
    expect(fetchMock.mock.calls.filter(([input]) => urlOf(input).pathname.endsWith("/access/attributes"))).toEqual([]);
  });

  // T-5600: a search that matched nothing said nothing had been written to the space.
  it("searches a name in any language, never a number, and says a search matched no row", async () => {
    const named = [{ ...SPACE_ROWS[0], name: { type: "LanguageProperty", languageMap: { fi: "Helsinki Herttoniemi 2" } } }];
    const asked: string[] = [];
    renderInside({ status: 200, count: 1 }, {
      status: 200,
      answer: (url) => {
        const q = url.searchParams.get("q");
        if (q) asked.push(q);
        return q?.includes("Nowhere") ? [] : named;
      },
    });
    await screen.findByRole("button", { name: `${en.entityGrid.openRow}: Helsinki Herttoniemi 2` });

    await userEvent.type(screen.getByRole("searchbox", { name: en.spaces.toolbar.search }), "Nowhere");
    const status = await screen.findByText(
      en.spaces.inside.searchNoMatch.replace("{type}", "AirQualityObserved").replace("{text}", "Nowhere"),
    );
    expect(screen.queryByText(en.spaces.inside.dataEmpty)).toBeNull();
    // Every search asked the name through its languages and left the number out, the empty page included.
    const searches = asked.filter((q) => q.includes("Nowhere"));
    expect(searches.length).toBeGreaterThan(0);
    expect(searches.every((q) => q.includes('name[*]~="(?i)Nowhere"') && !q.includes("pm10"))).toBe(true);

    await userEvent.click(within(status.closest("[role=status]") as HTMLElement).getByRole("button", { name: en.spaces.inside.clearSearch }));
    expect(await screen.findByRole("button", { name: `${en.entityGrid.openRow}: Helsinki Herttoniemi 2` })).toBeInTheDocument();
    expect(screen.getByRole("searchbox", { name: en.spaces.toolbar.search })).toHaveValue("");
  });

  it("says nothing has been written to an empty space", async () => {
    renderInside({ status: 200, count: 0 }, { status: 200, rows: [] });

    expect(await screen.findByText(en.spaces.inside.dataEmpty)).toBeInTheDocument();
  });

  it("folds an endpoint's representations into one line with a true count (T-2280)", async () => {
    renderInside({ status: 200, count: 1 });

    await openTab(en.endpoints.title);
    const endpointRow = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
    const folded = within(endpointRow).getByText(/ngsi-ld, csv/);
    // Eleven listed, eight served (the three retired files are not, T-4600), two named: the count is
    // what is left, never a fixed number.
    expect(within(endpointRow).getByText("+6 more")).toBeInTheDocument();
    // One line, and everything still reachable: a `details` is closed but its content is in the page
    // and its summary takes the keyboard on its own.
    const disclosure = folded.closest("details") as HTMLDetailsElement;
    expect(disclosure).toBeTruthy();
    expect(disclosure.open).toBe(false);
    expect(within(endpointRow).getByRole("link", { name: "csv" })).toBeInTheDocument();
    expect(within(endpointRow).getByRole("link", { name: en.endpoints.link.access })).toBeInTheDocument();

    // The count is arithmetic, not a constant: eight served minus the two named. An endpoint with
    // two or fewer therefore shows no count at all, which is the same branch (`rest > 0`).
    expect(within(endpointRow).getByText("ngsi-ld, csv")).toBeInTheDocument();
    expect(within(endpointRow).queryByText("+8 more")).toBeNull();
  });
});

describe("a type's samples (T-6156)", () => {
  it("lists each sample with its size, or says there is none", () => {
    const { unmount } = renderPart(<Samples samples={[{ id: "urn:ngsi-ld:Kpi:1", type: "Kpi", name: "District 1" }]} />);
    const list = screen.getByRole("list", { name: en.spaces.inside.samples });
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    expect(within(list).getByText("name: District 1")).toBeInTheDocument();
    expect(within(list).getByTitle(/^Size of the values shown, as JSON: \d+ B$/)).toBeInTheDocument();
    unmount();
    renderPart(<Samples samples={[]} />);
    expect(screen.getByText(en.spaces.inside.noEntities)).toBeInTheDocument();
    expect(screen.queryByRole("list")).not.toBeInTheDocument();
  });
});
