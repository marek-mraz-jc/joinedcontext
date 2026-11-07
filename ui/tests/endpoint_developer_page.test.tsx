// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/endpoints/DeveloperPanel.tsx, src/pages/endpoints/developerSamples.ts.
/**
 * T-5111: a public Endpoint's developer page. Licence and rate limit from the Portal's standards
 * answer, a ready sample per served surface, and try-it, sent anonymously below the Endpoint.
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { DeveloperPanel, pathProblem } from "../src/pages/endpoints/DeveloperPanel";
import { EndpointDocs } from "../src/pages/endpoints/EndpointDocs";
import { samplesOf, subscribeMessage } from "../src/pages/endpoints/developerSamples";

const SLUG = "rhnhewwotdngtrsb7gnzpj63ysnkutwl";
const BASE = `https://api.dev.example/api/endpoint/${SLUG}/`;
const FIRST = `${BASE}ngsi-ld/v1/entities?type=AirQualityObserved&limit=5`;
const ITEMS = `${BASE}ogc/features/collections/AirQualityObserved/items?limit=5`;

/** What `GET /api/v1/catalogue/endpoints/{slug}/standards` answers since T-5111. */
const STANDARDS = {
  slug: SLUG,
  types: ["AirQualityObserved"],
  entityType: "AirQualityObserved",
  base: BASE,
  served: ["ngsi-ld", "csv", "websocket", "ogc-features", "odata", "mcp"],
  licence: { code: "CC_BY_4_0", title: "Creative Commons Attribution 4.0", url: "http://publications.europa.eu/resource/authority/licence/CC_BY_4_0" },
  requestsPerMinute: 600,
  standards: [
    { representation: "ngsi-ld", fields: [], examples: [], first: FIRST },
    { representation: "ogc-features", fields: [], examples: [], first: ITEMS },
    { representation: "odata", fields: [], examples: [], first: null },
  ],
};

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

/** Every request the page sends; `answer` decides what the Endpoint itself says. */
function show(node: React.ReactNode, answer: (request: Request) => Response = () => json([{ id: "urn:ngsi-ld:AirQualityObserved:zv1" }]), standards = STANDARDS) {
  const seen: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input instanceof Request ? input : new Request(String(input));
      seen.push(request);
      const path = new URL(request.url).pathname;
      if (path.endsWith("/standards")) return json(standards);
      if (path.endsWith("/openapi.json")) {
        return json({
          openapi: "3.1.0",
          info: { title: "Air in Žilina" },
          paths: { "/ngsi-ld/v1/entities": { get: { summary: "The entities of a type", parameters: [{ name: "type", in: "query", schema: { enum: ["AirQualityObserved"] } }] } } },
        });
      }
      return answer(request);
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
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

describe("the samples of a developer page (T-5111)", () => {
  it("gives every served surface a sample on the Endpoint's own address, in the order a developer reads them", () => {
    const samples = samplesOf(STANDARDS);
    expect(samples.map((s) => `${s.surface} ${s.url}`)).toEqual([
      `ngsi-ld ${FIRST}`,
      `ogc-features ${ITEMS}`,
      `geojson ${ITEMS}`,
      `odata ${BASE}odata/v4/`,
      `csv ${BASE}file.csv`,
      `websocket wss://api.dev.example/api/endpoint/${SLUG}/ws`,
      `mcp ${BASE}mcp`,
    ]);
    const ngsi = samples[0].code;
    expect(ngsi.curl).toBe(`curl -sSf -H 'Accept: application/ld+json' '${FIRST}'`);
    expect(ngsi.python).toContain(`urllib.request.Request(${JSON.stringify(FIRST)}`);
    expect(ngsi.javascript).toContain(`await fetch(${JSON.stringify(FIRST)}`);
    expect(samples.find((s) => s.surface === "geojson")?.code.curl).toContain("-o data.geojson");
    expect(samples.find((s) => s.surface === "ogc-features")?.tools.map((t) => `${t.tool} ${t.url}`)).toEqual([
      `qgis ${BASE}ogc/features`,
      `arcgis ${BASE}ogc/features`,
    ]);
    expect(samples.find((s) => s.surface === "odata")?.tools).toEqual([{ tool: "excel", url: `${BASE}odata/v4/` }]);
    const socket = samples.find((s) => s.surface === "websocket");
    expect(socket?.code.curl).toBeUndefined();
    expect(socket?.code.javascript).toContain('"ngsi-ld-ws"');
    expect(samples.find((s) => s.surface === "mcp")?.code.curl).toContain('"method":"initialize"');
    // A public page never asks for a credential.
    expect(JSON.stringify(samples)).not.toMatch(/Authorization|TOKEN|Bearer/);
  });

  it("falls back to the surface's own index when no first page answered, and offers nothing it does not serve", () => {
    const samples = samplesOf({ base: BASE.slice(0, -1), served: ["ngsi-ld", "ogc-features", "sta", "ical"], standards: [] });
    expect(samples.map((s) => `${s.surface} ${s.url}`)).toEqual([
      `ngsi-ld ${BASE}ngsi-ld/v1/types`,
      `ogc-features ${BASE}ogc/features/collections`,
      `sta ${BASE}sta/v1.1`,
      `ical ${BASE}ical/v1/events.ics`,
    ]);
    expect(samples[3].tools).toEqual([{ tool: "calendar", url: `webcal://api.dev.example/api/endpoint/${SLUG}/ical/v1/events.ics` }]);
  });

  it("quotes an address a shell would otherwise split", () => {
    const [sample] = samplesOf({ base: "https://api.dev.example/api/endpoint/x/", served: ["ngsi-ld"], standards: [{ representation: "ngsi-ld", first: "https://api.dev.example/api/endpoint/x/ngsi-ld/v1/entities?q=name=='O'Brien'" }] });
    expect(sample.code.curl).toContain(`'https://api.dev.example/api/endpoint/x/ngsi-ld/v1/entities?q=name=='"'"'O'"'"'Brien'"'"''`);
  });

  it("subscribes to the example type, or to every entity with a location", () => {
    expect(JSON.parse(subscribeMessage("Vehicle")).body).toEqual({ type: "Subscription", entities: [{ type: "Vehicle" }] });
    expect(JSON.parse(subscribeMessage(null)).body).toEqual({ type: "Subscription", watchedAttributes: ["location"] });
  });

  it("keeps a typed request below the Endpoint", () => {
    expect(pathProblem("/ngsi-ld/v1/types")).toBeUndefined();
    expect(pathProblem("/ngsi-ld/v1/entities/urn%3Angsi-ld%3AX%3A1?q=a..b")).toBeUndefined();
    expect(pathProblem("ngsi-ld/v1/types")).toBe("start");
    for (const outside of ["/../../api/v1/me", "/ngsi-ld/..", "//evil.example/x", "/%2e%2e/x", "/a%2Fb", "/a\\b", "/a#b"]) {
      expect(pathProblem(outside), outside).toBe("outside");
    }
  });
});

describe("the developer page (T-5111)", () => {
  it("names the licence, the rate limit and anonymous access", async () => {
    show(<DeveloperPanel slug={SLUG} />);
    const licence = await screen.findByTestId("developer-licence");
    expect(within(licence).getByRole("link", { name: "Creative Commons Attribution 4.0" })).toHaveAttribute("href", STANDARDS.licence.url);
    expect(screen.getByTestId("developer-limit")).toHaveTextContent("600 requests a minute per address");
    expect(screen.getByText(en.endpoints.developer.accessText)).toBeInTheDocument();
  });

  it("says so when the catalogue entry names no licence and no limit is known", async () => {
    show(<DeveloperPanel slug={SLUG} />, undefined, { ...STANDARDS, licence: null as never, requestsPerMinute: null as never });
    expect(await screen.findByTestId("developer-licence")).toHaveTextContent(en.endpoints.developer.noLicence);
    expect(screen.getByTestId("developer-limit")).toHaveTextContent(en.endpoints.developer.noLimit);
  });

  it("switches surface and language, and shows the desktop steps of a GIS surface", async () => {
    show(<DeveloperPanel slug={SLUG} />);
    await screen.findByTestId("developer-panel");
    const surfaces = screen.getByRole("tablist", { name: en.endpoints.developer.samples });
    expect(within(surfaces).getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["NGSI-LD", "OGC API Features", "GeoJSON", "OData", "CSV", "WebSocket", "MCP"]);
    fireEvent.click(within(surfaces).getByRole("tab", { name: "OGC API Features" }));
    expect(screen.getByRole("heading", { name: "QGIS" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "ArcGIS Pro" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Python" }));
    expect(screen.getByTestId("sample-ogc-features-python")).toHaveTextContent("urllib.request");
    fireEvent.click(within(surfaces).getByRole("tab", { name: "WebSocket" }));
    // No shell tool opens a WebSocket: the tab falls back to the first language it has.
    expect(screen.queryByRole("tab", { name: "curl" })).not.toBeInTheDocument();
    expect(screen.getByTestId("sample-websocket-python")).toHaveTextContent("ngsi-ld-ws");
  });

  it("sends a try-it below the Endpoint without the visitor's cookie, and shows the answer", async () => {
    const seen = show(<DeveloperPanel slug={SLUG} />);
    fireEvent.click(await screen.findByRole("button", { name: en.endpoints.developer.send }));
    expect(await screen.findByTestId("try-it-answer")).toHaveTextContent("urn:ngsi-ld:AirQualityObserved:zv1");
    expect(screen.getByRole("status")).toHaveTextContent("Answer: HTTP 200.");
    const sent = seen.find((request) => request.url.includes("/ngsi-ld/v1/entities"));
    expect(sent?.url).toBe(`${window.location.origin}/api/endpoint/${SLUG}/ngsi-ld/v1/entities?type=AirQualityObserved&limit=5`);
    expect(sent?.credentials).toBe("omit");
    expect(sent?.headers.get("Authorization")).toBeNull();
  });

  it("tells a visitor over the limit to wait", async () => {
    show(<DeveloperPanel slug={SLUG} />, () => json({ title: "Too Many Requests" }, 429));
    fireEvent.click(await screen.findByRole("button", { name: en.endpoints.developer.send }));
    expect(await screen.findByText(/too many requests\. Wait a minute/)).toBeInTheDocument();
  });

  it("shows nothing for an Endpoint the standards answer does not describe", async () => {
    show(<DeveloperPanel slug={SLUG} />, undefined, { ...STANDARDS, served: undefined as never });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByTestId("developer-panel")).not.toBeInTheDocument();
  });
});

describe("a developer's journey through the public docs page (T-5111)", () => {
  it("reads the terms, copies a sample and tries an edited OpenAPI call, refused outside the Endpoint", async () => {
    const seen = show(<EndpointDocs slug={SLUG} open />);
    expect(await screen.findByRole("heading", { level: 1, name: "Air in Žilina" })).toBeInTheDocument();
    expect(await screen.findByTestId("developer-limit")).toHaveTextContent("600");
    const operation = screen.getByRole("region", { name: "GET /ngsi-ld/v1/entities" });
    const request = within(operation).getByLabelText(en.endpoints.developer.request);
    expect(request).toHaveValue("/ngsi-ld/v1/entities?type=AirQualityObserved&limit=5");
    fireEvent.change(request, { target: { value: "/../../api/v1/me" } });
    expect(request).toHaveAccessibleDescription(expect.stringContaining(en.endpoints.developer.problem.outside));
    expect(within(operation).getByRole("button", { name: en.endpoints.developer.send })).toHaveAttribute("aria-disabled", "true");
    fireEvent.change(request, { target: { value: "/ngsi-ld/v1/types" } });
    fireEvent.click(within(operation).getByRole("button", { name: en.endpoints.developer.send }));
    expect(await within(operation).findByTestId("try-it-answer")).toBeInTheDocument();
    expect(seen.some((r) => r.url === `${window.location.origin}/api/endpoint/${SLUG}/ngsi-ld/v1/types` && r.credentials === "omit")).toBe(true);
    expect(seen.some((r) => r.url.includes("/api/v1/me"))).toBe(false);
  });

  it("offers no try-it on a signed-in Endpoint page, where calls carry the person's rights", async () => {
    show(<EndpointDocs slug={SLUG} open={false} />);
    await screen.findByRole("region", { name: "GET /ngsi-ld/v1/entities" });
    expect(screen.queryByRole("button", { name: en.endpoints.developer.send })).not.toBeInTheDocument();
    expect(screen.queryByTestId("developer-panel")).not.toBeInTheDocument();
  });
});
