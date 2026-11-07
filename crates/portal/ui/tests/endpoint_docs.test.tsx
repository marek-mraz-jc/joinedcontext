// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/endpoints/EndpointDocs.tsx, src/pages/endpoints/EndpointStandards.tsx.
/**
 * T-3265 (EP-99): an Endpoint's API documentation, read from the OpenAPI document the gateway
 * generates for the reader: each operation with its parameters and an example call, each type
 * with its attributes.
 */
import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { EndpointDocs, operationsOf, typesOf } from "../src/pages/endpoints/EndpointDocs";
import { EndpointStandards } from "../src/pages/endpoints/EndpointStandards";

const SLUG = "k4y7pq2mzt6vhx3nbwrs5cjd8f";
/** The shape `openapi_doc::document` answers, cut to two operations and one type. */
const DOC = {
  openapi: "3.1.0",
  info: { title: "Air quality in Banská Bystrica", description: "Stations and their readings.", version: "1.4.0" },
  servers: [{ url: `/api/endpoint/${SLUG}` }],
  paths: {
    "/ngsi-ld/v1/entities": {
      get: {
        summary: "The entities of a type",
        parameters: [
          { name: "type", in: "query", required: false, description: "The entity type", schema: { type: "string", enum: ["AirQualityObserved"] } },
          { name: "q", in: "query", required: false, description: "NGSI-LD query language", schema: { type: "string" } },
        ],
      },
    },
    "/ngsi-ld/v1/entities/{entityId}": {
      get: { summary: "One entity by its id", parameters: [{ name: "entityId", in: "path", required: true, description: "The entity's URN", schema: { type: "string" } }] },
    },
  },
  components: {
    schemas: {
      AirQualityObserved: { type: "object", properties: { pm10: { type: "number", description: "Particles under 10 µm" }, id: { type: "string" } } },
    },
  },
};

/** What `GET /api/v1/catalogue/endpoints/{slug}/standards` answers (API/01 §16a, T-4484). */
const STANDARDS = {
  slug: SLUG,
  types: ["AirQualityObserved"],
  entityType: "AirQualityObserved",
  standards: [
    {
      representation: "ngsi-ld",
      fields: ["name", "no2"],
      examples: [
        { kind: "equality", field: "name", url: `https://portal.test/api/endpoint/${SLUG}/ngsi-ld/v1/entities?type=AirQualityObserved&q=name%3D%3D%22Zvolen%22`, answered: 1 },
        { kind: "range", field: "no2", url: `https://portal.test/api/endpoint/${SLUG}/ngsi-ld/v1/entities?type=AirQualityObserved&q=no2%3E%3D21.5`, answered: 4 },
      ],
    },
    { representation: "odata", fields: [], examples: [] },
  ],
};

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

function show(answer: Response, open = true, standards: () => Response = () => json(STANDARDS)) {
  const seen: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input instanceof Request ? input : new Request(String(input));
      seen.push(request);
      return new URL(request.url).pathname.endsWith("/standards") ? standards() : answer;
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <EndpointDocs slug={SLUG} open={open} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return seen;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("an endpoint's documentation (T-3265)", () => {
  it("reads every operation with an example of the first type, and every type's attributes", () => {
    const operations = operationsOf(DOC);
    expect(operations.map((o) => `${o.method} ${o.path} ${o.example}`)).toEqual([
      "GET /ngsi-ld/v1/entities /ngsi-ld/v1/entities?type=AirQualityObserved&limit=5",
      "GET /ngsi-ld/v1/entities/{entityId} /ngsi-ld/v1/entities/urn%3Angsi-ld%3AAirQualityObserved%3A001",
    ]);
    expect(operations[0].parameters[0]).toEqual({ name: "type", where: "query", required: false, description: "The entity type", values: ["AirQualityObserved"] });
    expect(typesOf(DOC)).toEqual([
      { name: "AirQualityObserved", attributes: [{ name: "pm10", type: "number", description: "Particles under 10 µm" }, { name: "id", type: "string", description: "" }] },
    ]);
    expect(operationsOf({})).toEqual([]);
    expect(operationsOf({ paths: { "/ngsi-ld/v1/entities": { get: {} } } })[0].example).toBe("/ngsi-ld/v1/entities?limit=5");
  });

  it("shows the document as the gateway generated it, with an example call per operation", async () => {
    const seen = show(new Response(JSON.stringify(DOC), { headers: { "Content-Type": "application/json" } }));
    expect(await screen.findByRole("heading", { level: 1, name: "Air quality in Banská Bystrica" })).toBeInTheDocument();
    expect(new URL(seen[0].url).pathname).toBe(`/api/endpoint/${SLUG}/openapi.json`);
    const list = screen.getByRole("region", { name: "GET /ngsi-ld/v1/entities" });
    expect(within(list).getByRole("cell", { name: /The entity type \(AirQualityObserved\)/ })).toBeInTheDocument();
    expect(within(list).getByText(new RegExp(`curl .*/api/endpoint/${SLUG}/ngsi-ld/v1/entities\\?type=AirQualityObserved&limit=5`))).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "AirQualityObserved" })).toHaveTextContent("Particles under 10 µm");
  });

  it("asks the gateway in the page's language and heads with the title it answers (T-5630)", async () => {
    for (const [language, title] of [
      ["sk", "Kvalita ovzdušia v Banskej Bystrici"],
      ["en", "Air quality in Banská Bystrica"],
    ]) {
      await i18n.changeLanguage(language);
      vi.stubGlobal(
        "fetch",
        vi.fn(async (input: RequestInfo | URL) => {
          const request = input instanceof Request ? input : new Request(String(input));
          if (new URL(request.url).pathname.endsWith("/standards")) return json(STANDARDS);
          const slovak = request.headers.get("Accept-Language") === "sk";
          return json({ ...DOC, info: { ...DOC.info, title: slovak ? "Kvalita ovzdušia v Banskej Bystrici" : DOC.info.title } });
        }),
      );
      const { unmount } = render(
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <I18nextProvider i18n={i18n}>
            <EndpointDocs slug={SLUG} open />
          </I18nextProvider>
        </QueryClientProvider>,
      );
      expect(await screen.findByRole("heading", { level: 1, name: title })).toBeInTheDocument();
      unmount();
      vi.unstubAllGlobals();
    }
  });

  it("says it cannot be read when the gateway refuses, revealing nothing", async () => {
    show(new Response("{}", { status: 404 }));
    expect(await screen.findByText(en.endpoints.docs.unavailable)).toBeInTheDocument();
  });

  it("shows a public endpoint's standards with the examples that answered, read from the Portal", async () => {
    const seen = show(json(DOC));
    const panel = await screen.findByTestId("endpoint-standards");
    expect(seen.some((r) => new URL(r.url).pathname === `/api/v1/catalogue/endpoints/${SLUG}/standards`)).toBe(true);
    const ngsi = within(panel).getByRole("region", { name: en.endpoints.representationOption["ngsi-ld"] });
    expect(ngsi).toHaveTextContent("name, no2");
    const links = within(ngsi).getAllByRole("link");
    expect(links.map((l) => l.getAttribute("href"))).toEqual(STANDARDS.standards[0].examples.map((e) => e.url));
    expect(ngsi).toHaveTextContent("4 entities on the first page");
    expect(ngsi).toHaveTextContent("1 entity on the first page");
    const odata = within(panel).getByRole("region", { name: en.endpoints.representationOption.odata });
    expect(odata).toHaveTextContent(en.endpoints.standards.none);
  });

  it("renders on its own from the Portal's answer, with no OpenAPI document", async () => {
    show(json({}, 404));
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <EndpointStandards slug={SLUG} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(await screen.findAllByRole("heading", { name: en.endpoints.standards.title })).not.toHaveLength(0);
  });

  it("asks for no standards on an endpoint that is not public", async () => {
    const seen = show(json(DOC), false);
    await screen.findByRole("heading", { level: 1, name: "Air quality in Banská Bystrica" });
    expect(screen.queryByTestId("endpoint-standards")).toBeNull();
    expect(seen.some((r) => new URL(r.url).pathname.endsWith("/standards"))).toBe(false);
  });

  it("says the standards cannot be read when the Portal cannot answer, and keeps the rest", async () => {
    show(json(DOC), true, () => json({ title: "unavailable" }, 503));
    expect(await screen.findByText(en.endpoints.standards.unavailable)).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "GET /ngsi-ld/v1/entities" })).toBeInTheDocument();
  });

  // T-4565, red ci-full 39104708624: a 200 of another shape (a page's mock, an older Portal)
  // took the whole Endpoint page down through the ErrorBoundary.
  it("says the standards cannot be read when the answer is not the documented shape, and keeps the rest", async () => {
    show(json(DOC), true, () => json({ slug: SLUG }));
    expect(await screen.findByText(en.endpoints.standards.unavailable)).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "GET /ngsi-ld/v1/entities" })).toBeInTheDocument();
  });

  it("never links an example whose address is not http(s)", async () => {
    const hostile = structuredClone(STANDARDS);
    hostile.standards[0].examples[0].url = "javascript:alert(1)";
    show(json(DOC), true, () => json(hostile));
    const panel = await screen.findByTestId("endpoint-standards");
    expect(within(panel).getByText("javascript:alert(1)")).not.toHaveAttribute("href");
    expect(within(panel).getAllByRole("link")).toHaveLength(1);
  });
});
