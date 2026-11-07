/**
 * Charts on the dataset (T-4642, EP-130): an editor fills a template with the columns picked and
 * proposes the Endpoint with the chart, checked first; a reader sees the list and nothing to change.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { VisualizationsSection } from "../src/pages/endpoints/VisualizationsSection";
import { defaultData, withVisualizations } from "../src/pages/endpoints/visualizations";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a href="/changes">{children}</a>,
}));

const V = en.endpoints.page.visualizations;
const LINE = {
  $schema: "https://vega.github.io/schema/vega-lite/v5.json",
  data: { name: "source" },
  mark: "line",
  encoding: { x: { field: "dateObserved", type: "temporal" }, y: { field: "no2", type: "quantitative" } },
};
const BAR = {
  name: "no2-now",
  title: { en: "NO2 now" },
  description: { en: "Per station." },
  data: { source: "csv", type: "AirQualityObserved" },
  spec: { data: { name: "source" }, mark: "bar" },
};

function endpoint(catalog?: Record<string, unknown>): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "air", namespace: "ovzdusie" },
    spec: { contextSpaceRef: "ovzdusie", slug: "zt4qm7ge2xdv6ksb3ncf5arw2y", audience: "public", ...(catalog ? { catalog } : {}) },
    status: { phase: "Ready" },
  } as unknown as Manifest;
}

function respond(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

describe("VisualizationsSection", () => {
  let originalFetch: typeof global.fetch;
  let sent: { method: string; url: URL; body: string }[];

  beforeEach(async () => {
    originalFetch = global.fetch;
    sent = [];
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    global.fetch = originalFetch;
  });

  function mount(manifest: Manifest, canEdit: boolean) {
    global.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      const url = new URL(request.url);
      sent.push({ method: request.method, url, body: request.method === "GET" ? "" : await request.clone().text() });
      if (url.pathname.startsWith("/api/v1/visualization-templates/")) return respond(LINE);
      if (url.searchParams.get("dryRun")) return respond({ valid: true });
      return respond({ metadata: { name: "chg-1" }, status: { phase: "Proposed" } }, 202);
    }) as typeof fetch;
    render(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <VisualizationsSection
            project="ovzdusie"
            manifest={manifest}
            types={["AirQualityObserved"]}
            columnsOf={() => ["dateObserved", "no2", "name"]}
            served={["ngsi-ld", "csv"]}
            canEdit={canEdit}
          />
        </QueryClientProvider>
      </I18nextProvider>,
    );
  }

  it("fills the template with the columns picked and proposes the chart after a green check", async () => {
    mount(endpoint({ license: "cc-by-4.0", visualizations: [BAR] }), true);
    await userEvent.click(screen.getByText(V.add));
    const fill = screen.getByRole("button", { name: V.fill });
    expect(fill).toHaveAttribute("aria-disabled", "true");
    await userEvent.selectOptions(screen.getByLabelText(V.roles.x), "dateObserved");
    await userEvent.selectOptions(screen.getByLabelText(V.roles.y), "no2");
    await userEvent.click(fill);
    const get = sent.find((r) => r.method === "GET")!;
    expect(get.url.pathname).toBe("/api/v1/visualization-templates/line");
    expect(Object.fromEntries(get.url.searchParams)).toEqual({ x: "dateObserved", y: "no2" });
    await waitFor(() => expect((screen.getByLabelText(V.spec) as HTMLTextAreaElement).value).toContain("dateObserved"));

    await userEvent.type(screen.getByLabelText(V.name), "no2-day");
    await userEvent.type(screen.getByLabelText(V.chartTitle), "NO2 today");
    await userEvent.type(screen.getByLabelText(V.description), "Hourly mean.");
    await userEvent.click(screen.getByRole("button", { name: V.propose }));

    await waitFor(() => expect(sent.filter((r) => r.method === "PUT")).toHaveLength(2));
    const [check, put] = sent.filter((r) => r.method === "PUT");
    expect(check.url.searchParams.get("dryRun")).toBe("All");
    expect(put.url.pathname).toBe("/api/v1/projects/ovzdusie/endpoints/air");
    const body = JSON.parse(put.body);
    expect(body.status).toBeUndefined();
    expect(body.spec.catalog.license).toBe("cc-by-4.0");
    expect(body.spec.catalog.visualizations).toEqual([
      BAR,
      {
        name: "no2-day",
        title: { en: "NO2 today" },
        description: { en: "Hourly mean." },
        data: { source: "temporal", type: "AirQualityObserved", attrs: ["no2"], last: "PT24H", aggrMethod: "avg", aggrPeriod: "PT1H" },
        spec: LINE,
      },
    ]);
  });

  it("refuses a name already taken and a spec that is not JSON before anything is sent", async () => {
    mount(endpoint({ visualizations: [BAR] }), true);
    await userEvent.click(screen.getByText(V.add));
    await userEvent.type(screen.getByLabelText(V.name), "no2-now");
    await userEvent.type(screen.getByLabelText(V.chartTitle), "T");
    await userEvent.type(screen.getByLabelText(V.description), "D");
    await userEvent.type(screen.getByLabelText(V.spec), "not json");
    expect(screen.getByRole("button", { name: V.propose })).toHaveAttribute("aria-disabled", "true");
    await userEvent.clear(screen.getByLabelText(V.name));
    await userEvent.type(screen.getByLabelText(V.name), "other");
    await userEvent.click(screen.getByRole("button", { name: V.propose }));
    expect(screen.getByRole("alert")).toHaveTextContent(V.badJson);
    expect(sent).toHaveLength(0);
  });

  it("shows a reader the charts and nothing to change", () => {
    mount(endpoint({ visualizations: [BAR] }), false);
    expect(screen.getByText("NO2 now")).toBeInTheDocument();
    expect(screen.queryByText(V.add)).toBeNull();
    expect(screen.queryByRole("button", { name: V.remove })).toBeNull();
  });

  it("removes a chart, and the catalogue block with it when nothing else is left", async () => {
    mount(endpoint({ visualizations: [BAR] }), true);
    await userEvent.click(screen.getByRole("button", { name: V.remove }));
    await waitFor(() => expect(sent.filter((r) => r.method === "PUT")).toHaveLength(2));
    expect(JSON.parse(sent[1].body).spec.catalog).toBeUndefined();
  });
});

describe("visualization data", () => {
  it("reads the history where the Endpoint serves NGSI-LD, else the CSV, else nothing", () => {
    expect(defaultData("bar", "T", "v", ["odata", "csv"])).toEqual({ source: "odata", entitySet: "T" });
    expect(defaultData("line", "T", "v", ["csv"])).toEqual({ source: "csv", type: "T" });
    expect(defaultData("heatmap", "T", "v", ["ogc-features"])).toBeUndefined();
  });

  it("keeps the rest of the catalogue when the list empties", () => {
    const next = withVisualizations(endpoint({ license: "cc0", visualizations: [BAR] }), []);
    expect((next.spec as { catalog: unknown }).catalog).toEqual({ license: "cc0" });
  });
});
