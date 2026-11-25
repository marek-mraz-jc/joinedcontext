// T-7706: VisualizationsSection edges: a template that cannot be filled, no served source, another type, a refused layered link and its repair.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { VisualizationsSection } from "../src/pages/endpoints/VisualizationsSection";
import { doorsAsRest } from "./odata";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a href="/changes">{children}</a>,
}));

const V = en.endpoints.page.visualizations;
const marked = (label: string) => new RegExp(`^${label}\\*?$`);
const URL_LINK = "https://example.org/no2.csv";
const LAYERED = {
  $schema: "https://vega.github.io/schema/vega-lite/v5.json",
  layer: [{ data: { url: URL_LINK }, mark: "line", encoding: { x: { field: "dateObserved", type: "temporal" }, y: { field: "no2", type: "quantitative" } } }],
};
const MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "air", namespace: "ovzdusie" },
  spec: { contextSpaceRef: "ovzdusie", slug: "zt4qm7ge2xdv6ksb3ncf5arw2y", audience: "public" },
  status: { phase: "Ready" },
} as unknown as Manifest;

const respond = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

describe("VisualizationsSection, the edges", () => {
  let sent: { method: string; url: URL; body: string }[];
  let templateRefused: boolean;
  let refusal: string | undefined;

  beforeEach(async () => {
    sent = [];
    templateRefused = false;
    refusal = undefined;
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  function mount({ served = ["ngsi-ld", "csv"], types = ["AirQualityObserved"] }: { served?: string[]; types?: string[] } = {}) {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
        const url = new URL(request.url);
        sent.push({ method: request.method, url, body: request.method === "GET" ? "" : await request.clone().text() });
        if (url.pathname.startsWith("/api/v1/visualization-templates/")) {
          return templateRefused ? respond({ title: "Gone", status: 503, detail: "the template store is away" }, 503) : respond(LAYERED);
        }
        if (request.method === "PUT" && refusal && url.searchParams.get("dryRun")) {
          return respond({ valid: false, verdict: { ok: false, findings: [{ path: "", message: refusal }] } });
        }
        if (url.searchParams.get("dryRun")) return respond({ valid: true });
        return respond({ metadata: { name: "chg-1" }, status: { phase: "Proposed" } }, 202);
      }),
    );
    doorsAsRest();
    render(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <VisualizationsSection project="ovzdusie" manifest={MANIFEST} types={types} columnsOf={() => ["dateObserved", "no2", "name"]} served={served} canEdit />
        </QueryClientProvider>
      </I18nextProvider>,
    );
  }

  async function fillForm(user: ReturnType<typeof userEvent.setup>) {
    await user.click(screen.getByText(V.add));
    await user.selectOptions(screen.getByLabelText(marked(V.roles.y)), "no2");
    await user.type(screen.getByLabelText(marked(V.name)), "no2-new");
    await user.type(screen.getByLabelText(marked(V.chartTitle)), "New");
    await user.type(screen.getByLabelText(marked(V.description)), "New.");
  }

  it("says why the template could not be filled and proposes nothing when Propose has no spec to send", async () => {
    const user = userEvent.setup();
    templateRefused = true;
    mount();
    await fillForm(user);
    await user.click(screen.getByRole("button", { name: V.fill }));
    expect(await screen.findByRole("alert")).toHaveTextContent("the template store is away");
    await user.click(screen.getByRole("button", { name: V.propose }));
    await waitFor(() => expect(sent.filter((r) => r.url.pathname.startsWith("/api/v1/visualization-templates/"))).toHaveLength(2));
    expect(sent.some((r) => r.method === "PUT")).toBe(false);
  });

  it("says the endpoint serves no source a chart could read, and sends nothing", async () => {
    const user = userEvent.setup();
    mount({ served: ["mqtt"] });
    await fillForm(user);
    // With no history to read, the axis is not defaulted.
    await user.selectOptions(screen.getByLabelText(marked(V.roles.x)), "dateObserved");
    await user.click(screen.getByRole("button", { name: V.propose }));
    expect(await screen.findByRole("alert")).toHaveTextContent(V.noSource);
    expect(sent.some((r) => r.method === "PUT")).toBe(false);
  });

  it("clears the picked columns when another entity type is chosen", async () => {
    const user = userEvent.setup();
    mount({ types: ["AirQualityObserved", "NoiseLevelObserved"] });
    await user.click(screen.getByText(V.add));
    const measure = screen.getByLabelText(marked(V.roles.y));
    await user.selectOptions(measure, "no2");
    expect(measure).toHaveValue("no2");
    await user.selectOptions(screen.getByLabelText(marked(V.type)), "NoiseLevelObserved");
    expect(screen.getByLabelText(marked(V.type))).toHaveValue("NoiseLevelObserved");
    expect(screen.getByLabelText(marked(V.roles.y))).toHaveValue("");
  });

  async function proposeLayered(user: ReturnType<typeof userEvent.setup>) {
    await fillForm(user);
    await user.click(screen.getByText(V.advanced));
    const spec = screen.getByLabelText(V.spec) as HTMLTextAreaElement;
    await user.click(spec);
    await user.paste(JSON.stringify(LAYERED));
    refusal = "spec is not a valid Endpoint: invalid /spec/catalog/visualizations/0/spec/layer/0/data/url: data-source: never a URL";
    await user.click(screen.getByRole("button", { name: V.propose }));
    await screen.findByRole("button", { name: V.useSource });
    return spec;
  }

  it("replaces the refused link of a layer with this endpoint's data and keeps the rest of the edit", async () => {
    const user = userEvent.setup();
    mount();
    const spec = await proposeLayered(user);
    await user.click(screen.getByRole("button", { name: V.useSource }));
    const edited = JSON.parse(spec.value) as { layer: { data: unknown; mark: string }[] };
    expect(edited.layer[0].data).toEqual({ name: "source" });
    expect(edited.layer[0].mark).toBe("line");
  });

  it("goes to nothing when the refused link is no longer in the text, and says a broken edit is not JSON", async () => {
    const user = userEvent.setup();
    mount();
    const spec = await proposeLayered(user);
    await user.clear(spec);
    await user.click(spec);
    await user.paste("{ not json");
    await user.click(screen.getByRole("button", { name: V.showField }));
    expect(spec.selectionStart).toBe(spec.selectionEnd);
    await user.click(screen.getByRole("button", { name: V.useSource }));
    expect(await screen.findByText(V.badJson)).toBeInTheDocument();
  });
});
