import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { ConstraintList, Operations, readsOf, sourcesOf, writesOf } from "../src/pages/access/EffectivePermissions";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  roles: ["portal-viewer"],
};

const ENDPOINTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "public-air", namespace: "banskabystrica" },
      spec: { slug: "mluyob4nz52lok3ssk7pgn5vwt", audience: "public" },
    },
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "parking-internal", namespace: "banskabystrica" },
      spec: { slug: "zt4qm7ge2xdv6ksb3ncf5arw2y", audience: "organization" },
    },
  ],
};

/** The document the Context Gateway serves on `…/access` (EP-55, EP-56). */
const GRANTS = {
  subject: { type: "user", id: "did:web:banskabystrica.sk:users:janko" },
  resource: { type: "endpoint", id: "mluyob4nz52lok3ssk7pgn5vwt", space: "ovzdusie" },
  permissions: [
    {
      resource: { type: "AirQualityObserved" },
      actions: ["retrieveEntity", "queryEntity", "updateAttrs"],
      attributes: ["pm10", "pm25", "location"],
      constraints: {
        geoQ: "georel=within;geometry=Polygon;coordinates=[[1,2]]",
        temporalQ: "timerel=after;timeAt=P-1D",
      },
    },
    {
      resource: { type: "AirQualityObserved" },
      actions: ["queryTemporal"],
      attributes: "*",
      constraints: { q: "pm10>=40;stationName==\"Centrum\"" },
    },
    {
      resource: { type: "District", idPatterns: ["urn:ngsi-ld:District:bb:.*"] },
      actions: ["retrieveEntity"],
      attributes: "*",
      constraints: {},
    },
  ],
  prohibitions: [
    {
      resource: { type: "PersonRecord" },
      actions: ["retrieveEntity"],
      attributes: "*",
      constraints: {},
    },
  ],
};

/** The caller's own Policy list: one matches the temporal grant, none the `q` one. */
const POLICIES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Policy",
      metadata: { name: "air-last-day", namespace: "banskabystrica" },
      spec: {
        contextSpaceRef: "ovzdusie",
        assignee: { kind: "role", id: "viewer" },
        operations: ["retrieveEntity", "queryEntity", "updateAttrs"],
        information: [{ entities: [{ type: "AirQualityObserved" }], propertyNames: ["pm10", "pm25", "location"] }],
        geoQ: "georel=within;geometry=Polygon;coordinates=[[1,2]]",
        temporalQ: "timerel=after;timeAt=P-1D",
      },
    },
  ],
};

function renderMatrix(document: unknown = GRANTS, accessStatus = 200) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    // The generated client hands over a `Request`; the gateway call is a plain `fetch(url)`.
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const href = request ? request.url : String(input);
    const path = new URL(href, window.location.origin).pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/access")) {
      return json(document, accessStatus);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    if (path.endsWith("/policies")) {
      return json(POLICIES);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
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

/** The path of one recorded call, whichever shape the caller used. */
const pathOf = (input: unknown): string => {
  const href =
    typeof input === "string" || input instanceof URL ? String(input) : (input as Request).url;
  return new URL(href, window.location.origin).pathname;
};

const rowOf = async (type: string) =>
  (await screen.findByRole("rowheader", { name: new RegExp(type) })).closest(
    "tr",
  ) as HTMLElement;

describe("effective permissions", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/settings/access");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("splits CIM 009 operations into reads and writes", () => {
    expect(readsOf(["retrieveEntity", "queryEntity", "updateAttrs"])).toEqual([
      "retrieveEntity",
      "queryEntity",
    ]);
    expect(writesOf(["retrieveEntity", "updateAttrs", "deleteEntity"])).toEqual([
      "updateAttrs",
      "deleteEntity",
    ]);
    expect(writesOf(["queryTemporal"])).toEqual([]);
  });

  it("names operations by area, the CIM 009 names behind them, writes in the warning tone", () => {
    const { container, rerender } = render(
      <I18nextProvider i18n={i18n}>
        <Operations actions={["retrieveEntity", "queryEntity", "someFutureOp"]} />
      </I18nextProvider>,
    );
    const items = container.querySelectorAll("li");
    expect(items[0]).toHaveAttribute("title", "retrieveEntity, queryEntity");
    expect(container).toHaveTextContent("(retrieveEntity, queryEntity)");
    expect(screen.getByText("someFutureOp")).toBeInTheDocument();
    expect(container.querySelector(".text-warning")).toBeNull();

    rerender(
      <I18nextProvider i18n={i18n}>
        <Operations actions={["updateAttrs"]} tone="warning" />
      </I18nextProvider>,
    );
    expect(container.querySelector(".text-warning")).not.toBeNull();

    rerender(
      <I18nextProvider i18n={i18n}>
        <Operations actions={[]} />
      </I18nextProvider>,
    );
    expect(container).toHaveTextContent(en.access.matrix.none);
  });

  it("lists each filter of a grant with the Policy's own text, or says it has none", () => {
    const { container, rerender } = render(
      <I18nextProvider i18n={i18n}>
        <ConstraintList entry={{ constraints: { q: "pm10>40", scopeQ: "/city/#" } }} labels={{}} />
      </I18nextProvider>,
    );
    const items = container.querySelectorAll("li");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveAttribute("title", "q=pm10>40");
    expect(items[0]).toHaveTextContent("(q=pm10>40)");
    expect(items[1]).toHaveAttribute("title", "scopeQ=/city/#");

    rerender(
      <I18nextProvider i18n={i18n}>
        <ConstraintList entry={{ constraints: { q: "" } }} labels={{}} />
      </I18nextProvider>,
    );
    expect(container.querySelector("li")).toBeNull();
    expect(container).toHaveTextContent(en.access.matrix.unconstrained);
  });

  it("renders one row group per entity type with its operations, attributes and filters in words", async () => {
    renderMatrix();

    const air = await rowOf("AirQualityObserved");
    expect(within(air).getByText(en.access.words.area.read)).toBeInTheDocument();
    expect(within(air).getByText(en.access.words.area.change)).toBeInTheDocument();
    expect(within(air).getByText("(retrieveEntity, queryEntity)")).toBeInTheDocument();
    expect(within(air).getByText("(updateAttrs)")).toBeInTheDocument();
    // An attribute as a chip, its name on hover (T-5593).
    expect(within(air).getByTitle("pm10")).toBeInTheDocument();
    // T-5593: the filters a person reads, the NGSI-LD text only on hover and to a screen reader.
    expect(within(air).getByText("inside a drawn area")).toBeInTheDocument();
    expect(within(air).getByText("history up to 1 day back")).toBeInTheDocument();
    expect(within(air).getByTitle("temporalQ=timerel=after;timeAt=P-1D")).toBeInTheDocument();
    // Two grants on one type say so, and each names the Policy it comes from.
    expect(within(air).getByText("2 grants")).toBeInTheDocument();
    expect(await within(air).findByRole("link", { name: "air-last-day" })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/policies/air-last-day/edit",
    );
    const second = air.nextElementSibling as HTMLElement;
    expect(within(second).getByText('where pm10 ≥ 40, stationName = "Centrum"')).toBeInTheDocument();
    expect(within(second).getByText(en.access.matrix.noSource)).toBeInTheDocument();

    const district = await rowOf("District");
    expect(within(district).getByText(en.access.matrix.allAttributes)).toBeInTheDocument();
    expect(within(district).getByText(en.access.matrix.unconstrained)).toBeInTheDocument();
    expect(within(district).getByText(/urn:ngsi-ld:District:bb/)).toBeInTheDocument();
    // Read-only: nothing in the write column of a type the caller may only read.
    expect(within(district).getAllByText(en.access.matrix.none).length).toBeGreaterThan(0);
  });

  it("reads the endpoint's own access surface, never a second implementation (EP-60)", async () => {
    const fetchMock = renderMatrix();
    await screen.findByRole("rowheader", { name: /AirQualityObserved/ });

    const asked = fetchMock.mock.calls
      .map((call) => pathOf(call[0]))
      .find((path) => path.endsWith("/access"));
    expect(asked).toBe("/api/endpoint/mluyob4nz52lok3ssk7pgn5vwt/access");
  });

  it("opens on the Endpoint ?endpoint= names, and only one the caller's list holds (T-5593)", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/settings/access?endpoint=parking-internal");
    const fetchMock = renderMatrix();
    expect(await screen.findByLabelText(en.access.matrix.endpoint)).toHaveValue("zt4qm7ge2xdv6ksb3ncf5arw2y");
    await vi.waitFor(() => {
      const paths = fetchMock.mock.calls.map((call) => pathOf(call[0]));
      expect(paths).toContain("/api/endpoint/zt4qm7ge2xdv6ksb3ncf5arw2y/access");
      expect(paths).not.toContain("/api/endpoint/mluyob4nz52lok3ssk7pgn5vwt/access");
    });
  });

  it("falls back to the first Endpoint for a name the list does not hold", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/settings/access?endpoint=someone-elses");
    renderMatrix();
    expect(await screen.findByLabelText(en.access.matrix.endpoint)).toHaveValue("mluyob4nz52lok3ssk7pgn5vwt");
  });

  it("matches a grant to its Policy by space, effect, type, attributes and filters", () => {
    const entry = { resource: { type: "AirQualityObserved" }, attributes: "*" as const, constraints: { q: "pm10>1" } };
    const policy = (name: string, spec: Record<string, unknown>) => ({ name, spec: { contextSpaceRef: "ovzdusie", ...spec } });
    const air = { information: [{ entities: [{ type: "AirQualityObserved" }] }] };
    expect(
      sourcesOf(
        entry,
        [
          policy("same", { ...air, q: "pm10>1" }),
          policy("other-filter", { ...air, q: "pm10>2" }),
          policy("other-space", { ...air, q: "pm10>1", contextSpaceRef: "doprava" }),
          policy("denies", { ...air, q: "pm10>1", effect: "prohibition" }),
          policy("by-rule", { rules: [{ ...air, q: "pm10>1" }] }),
          policy("named-attributes", { information: [{ entities: [{ type: "AirQualityObserved" }], propertyNames: ["pm10"] }], q: "pm10>1" }),
        ],
        "ovzdusie",
        "permission",
      ),
    ).toEqual(["same", "by-rule"]);
  });

  it("asks the endpoint the reader picks", async () => {
    const fetchMock = renderMatrix();
    await screen.findByRole("rowheader", { name: /AirQualityObserved/ });

    await userEvent.selectOptions(
      screen.getByLabelText(en.access.matrix.endpoint),
      "zt4qm7ge2xdv6ksb3ncf5arw2y",
    );

    await vi.waitFor(() => {
      const paths = fetchMock.mock.calls.map((call) => pathOf(call[0]));
      expect(paths).toContain("/api/endpoint/zt4qm7ge2xdv6ksb3ncf5arw2y/access");
    });
    // The address keeps the choice, so a reload or a shared link opens the same table (T-5593).
    expect(window.location.search).toBe("?endpoint=parking-internal");
  });

  it("shows denials the document names and nothing about types it does not (EP-59)", async () => {
    renderMatrix();

    expect(
      await screen.findByRole("heading", { name: en.access.matrix.prohibitions }),
    ).toBeInTheDocument();
    expect(screen.getByRole("rowheader", { name: /PersonRecord/ })).toBeInTheDocument();
    // A type no policy names is simply absent: the view must not invent a "denied" row for it.
    expect(screen.queryByText(/ParkingSpot/)).not.toBeInTheDocument();
  });

  it("says nothing is readable rather than showing an empty table", async () => {
    renderMatrix({ subject: { id: "anonymous" }, permissions: [], prohibitions: [] });

    expect(await screen.findByText(en.access.matrix.empty)).toBeInTheDocument();
    // The Access page also lists the people and their roles; the data access matrix draws none.
    expect(screen.queryByRole("table", { name: en.access.matrix.title })).not.toBeInTheDocument();
  });

  it("explains a refusal instead of rendering a broken table", async () => {
    renderMatrix({ status: 403 }, 403);

    expect(await screen.findByText(en.access.matrix.forbidden)).toBeInTheDocument();
  });
});
