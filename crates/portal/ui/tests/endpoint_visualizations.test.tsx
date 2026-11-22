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
import {
  defaultData,
  withVisualizations,
} from "../src/pages/endpoints/visualizations";
import { doorsAsRest } from "./odata";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ children }: { children: React.ReactNode }) => (
    <a href="/changes">{children}</a>
  ),
}));

const V = en.endpoints.page.visualizations;
const LINE = {
  $schema: "https://vega.github.io/schema/vega-lite/v5.json",
  data: { name: "source" },
  mark: "line",
  encoding: {
    x: { field: "dateObserved", type: "temporal" },
    y: { field: "no2", type: "quantitative" },
  },
};
const BAR = {
  name: "no2-now",
  title: { en: "NO2 now" },
  description: { en: "Per station." },
  data: { source: "csv", type: "AirQualityObserved" },
  spec: { data: { name: "source" }, mark: "bar" },
};

const WEEK = {
  name: "no2-week",
  title: { en: "NO2 week" },
  description: { en: "Daily peak." },
  data: { source: "temporal", type: "AirQualityObserved", attrs: ["no2"], last: "P7D", aggrMethod: "max", aggrPeriod: "P1D" },
  spec: LINE,
};

function endpoint(catalog?: Record<string, unknown>): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "air", namespace: "ovzdusie" },
    spec: {
      contextSpaceRef: "ovzdusie",
      slug: "zt4qm7ge2xdv6ksb3ncf5arw2y",
      audience: "public",
      ...(catalog ? { catalog } : {}),
    },
    status: { phase: "Ready" },
  } as unknown as Manifest;
}

function respond(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

describe("VisualizationsSection", () => {
  let sent: { method: string; url: URL; body: string }[];

  let refusal: string[] | undefined;
  let passesCheck: boolean;

  beforeEach(async () => {
    sent = [];
    refusal = undefined;
    passesCheck = false;
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function mount(manifest: Manifest, canEdit: boolean) {
    vi.stubGlobal("fetch", vi.fn(
      async (input: RequestInfo | URL, init?: RequestInit) => {
        const request =
          input instanceof Request
            ? input
            : new Request(new URL(String(input), window.location.origin), init);
        const url = new URL(request.url);
        sent.push({
          method: request.method,
          url,
          body: request.method === "GET" ? "" : await request.clone().text(),
        });
        // The check answers a verdict whose findings carry the pointer in the sentence, no path.
        if (request.method === "PUT" && refusal && !passesCheck && url.searchParams.get("dryRun"))
          return respond({ valid: false, verdict: { ok: false, findings: refusal.map((message) => ({ path: "", message })) } });
        if (request.method === "PUT" && refusal && !url.searchParams.get("dryRun"))
          return respond({ title: "Not valid", status: 422, detail: refusal.join("\n"), errors: refusal }, 422);
        if (url.pathname.startsWith("/api/v1/visualization-templates/"))
          return respond(LINE);
        if (url.searchParams.get("dryRun")) return respond({ valid: true });
        return respond(
          { metadata: { name: "chg-1" }, status: { phase: "Proposed" } },
          202,
        );
      },
    ));
    render(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider
          client={
            new QueryClient({ defaultOptions: { queries: { retry: false } } })
          }
        >
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
    doorsAsRest();
    await userEvent.click(screen.getByText(V.add));
    const fill = screen.getByRole("button", { name: V.fill });
    expect(fill).toHaveAttribute("aria-disabled", "true");
    await userEvent.selectOptions(
      screen.getByLabelText(V.roles.x),
      "dateObserved",
    );
    await userEvent.selectOptions(screen.getByLabelText(V.roles.y), "no2");
    await userEvent.click(fill);
    const get = sent.find((r) => r.method === "GET")!;
    expect(get.url.pathname).toBe("/api/v1/visualization-templates/line");
    expect(Object.fromEntries(get.url.searchParams)).toEqual({
      x: "dateObserved",
      y: "no2",
    });
    await waitFor(() =>
      expect(
        (screen.getByLabelText(V.spec) as HTMLTextAreaElement).value,
      ).toContain("dateObserved"),
    );

    await userEvent.type(screen.getByLabelText(V.name), "no2-day");
    await userEvent.type(screen.getByLabelText(V.chartTitle), "NO2 today");
    await userEvent.type(screen.getByLabelText(V.description), "Hourly mean.");
    await userEvent.click(screen.getByRole("button", { name: V.propose }));

    await waitFor(() =>
      expect(sent.filter((r) => r.method === "PUT")).toHaveLength(2),
    );
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
        data: {
          source: "temporal",
          type: "AirQualityObserved",
          attrs: ["no2"],
          last: "PT24H",
          aggrMethod: "avg",
          aggrPeriod: "PT1H",
        },
        spec: LINE,
      },
    ]);
  });

  it("proposes the reader's toolbar defaults and switch picked for a temporal chart (T-7491)", async () => {
    mount(endpoint(), true);
    doorsAsRest();
    await userEvent.click(screen.getByText(V.add));
    const toolbar = screen.getByTestId("viz-temporal");
    expect(toolbar).toHaveAccessibleName(V.toolbar);
    // Each field has its hint with an example, read after the control's name.
    expect(screen.getByLabelText(V.range)).toHaveAccessibleDescription(V.hint.range);
    expect(screen.getByLabelText(V.granularity)).toHaveAccessibleDescription(V.hint.granularity);
    expect(screen.getByLabelText(V.method)).toHaveAccessibleDescription(V.hint.method);
    expect(screen.getByLabelText(V.controls)).toHaveAccessibleDescription(V.hint.controls);
    // Only the allow-listed values are offered.
    const options = (label: string) =>
      Array.from((screen.getByLabelText(label) as HTMLSelectElement).options).map((o) => o.text);
    expect(options(V.range)).toEqual([V.ranges.day, V.ranges.week, V.ranges.month]);
    expect(options(V.granularity)).toEqual([V.granularities.raw, V.granularities.hourly, V.granularities.daily]);
    expect(options(V.method)).toEqual([V.methods.avg, V.methods.min, V.methods.max]);

    // Raw readings take no method: the field goes and the data block holds no aggregation.
    await userEvent.selectOptions(screen.getByLabelText(V.granularity), "raw");
    expect(screen.queryByLabelText(V.method)).toBeNull();
    await userEvent.selectOptions(screen.getByLabelText(V.granularity), "daily");
    await userEvent.selectOptions(screen.getByLabelText(V.range), "week");
    await userEvent.selectOptions(screen.getByLabelText(V.method), "max");
    await userEvent.click(screen.getByLabelText(V.controls));

    await userEvent.selectOptions(screen.getByLabelText(V.roles.x), "dateObserved");
    await userEvent.selectOptions(screen.getByLabelText(V.roles.y), "no2");
    await userEvent.click(screen.getByRole("button", { name: V.fill }));
    await waitFor(() =>
      expect((screen.getByLabelText(V.spec) as HTMLTextAreaElement).value).toContain("dateObserved"),
    );
    await userEvent.type(screen.getByLabelText(V.name), "no2-week");
    await userEvent.type(screen.getByLabelText(V.chartTitle), "NO2 this week");
    await userEvent.type(screen.getByLabelText(V.description), "Daily peak.");
    await userEvent.click(screen.getByRole("button", { name: V.propose }));
    await waitFor(() => expect(sent.filter((r) => r.method === "PUT")).toHaveLength(2));
    const put = sent.filter((r) => r.method === "PUT")[1];
    expect(JSON.parse(put.body).spec.catalog.visualizations[0].data).toEqual({
      source: "temporal",
      type: "AirQualityObserved",
      attrs: ["no2"],
      last: "P7D",
      aggrMethod: "max",
      aggrPeriod: "P1D",
      controls: false,
    });
  });

  async function proposeLine() {
    await userEvent.click(screen.getByText(V.add));
    await userEvent.type(screen.getByLabelText(V.name), "no2-new");
    await userEvent.type(screen.getByLabelText(V.chartTitle), "New");
    await userEvent.type(screen.getByLabelText(V.description), "New.");
    await userEvent.click(screen.getByLabelText(V.spec));
    await userEvent.paste(JSON.stringify(LINE));
    await userEvent.click(screen.getByRole("button", { name: V.propose }));
  }

  it.each([
    ["last", "range", "24 hours was not accepted as this chart's range. Pick 7 days or 31 days."],
    ["aggrPeriod", "granularity", "Hourly was not accepted as this chart's granularity. Pick Raw readings or Daily."],
    ["aggrMethod", "method", "Average was not accepted as this chart's method. Pick Minimum or Maximum."],
    ["controls", "controls", V.fix.controls],
  ] as const)("says what to pick on the refused %s of the chart being added, without the pointer (T-7491)", async (member, field, says) => {
    mount(endpoint({ visualizations: [BAR] }), true);
    doorsAsRest();
    refusal = [`invalid /spec/catalog/visualizations/1/data/${member}: source: not allowed`];
    await proposeLine();
    const control = screen.getByLabelText(V[field]);
    await waitFor(() => expect(control).toHaveAttribute("aria-invalid", "true"));
    expect(control).toHaveAccessibleDescription(expect.stringContaining(says));
    if (field !== "controls") expect(control).not.toHaveAccessibleDescription(expect.stringContaining(V.hint[field]));
    expect(document.body.textContent).not.toContain("/spec/catalog");
    expect(screen.getAllByText(new RegExp(`Nothing was proposed.*${V[field]}`))).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: V.showField }));
    expect(control).toHaveFocus();
  });

  it("leaves the new chart's fields alone when another chart is the one refused (T-7491)", async () => {
    mount(endpoint({ visualizations: [BAR, WEEK] }), true);
    doorsAsRest();
    refusal = ["/spec/catalog/visualizations/1/data/last: source: not allowed"];
    await proposeLine();
    expect(await screen.findByText(/Nothing was proposed.*NO2 week/)).toBeVisible();
    expect(screen.getByText(/^7 days is not an allowed range/)).toBeVisible();
    expect(screen.getByLabelText(V.range)).not.toHaveAttribute("aria-invalid");
    expect(screen.queryByRole("button", { name: V.showField })).toBeNull();
  });

  it("names the chart that blocks a removal, by its place in what was sent, outside the closed editor (T-7491)", async () => {
    mount(endpoint({ visualizations: [BAR, WEEK] }), true);
    doorsAsRest();
    refusal = ["/spec/catalog/visualizations/0/data/last: source: not allowed"];
    await userEvent.click(screen.getAllByRole("button", { name: V.remove })[0]);
    const alert = await screen.findByText(/“NO2 now” was not removed, because “NO2 week”/);
    expect(screen.getByText(/^7 days is not an allowed range/).closest("li")).toHaveTextContent("no2-week");
    expect(alert).toBeVisible();
    expect(alert.closest("details")).toBeNull();
    expect(screen.getByLabelText(V.range, { selector: "select" })).not.toHaveAttribute("aria-invalid");
  });

  it("reads the proposal's own 422 as it reads the check's verdict (T-7491)", async () => {
    mount(endpoint(), true);
    doorsAsRest();
    passesCheck = true;
    refusal = [
      "/spec/catalog/visualizations/0/data/last: source: not allowed",
      "/spec/catalog/visualizations/0/data/aggrMethod: not allowed",
    ];
    await proposeLine();
    await waitFor(() => expect(screen.getByLabelText(V.range)).toHaveAttribute("aria-invalid", "true"));
    expect(screen.getByLabelText(V.method)).toHaveAttribute("aria-invalid", "true");
    expect(document.body.textContent).not.toContain("/spec/catalog");
  });

  it("keeps the server's sentence for a refusal it has no words for (T-7491)", async () => {
    mount(endpoint(), true);
    doorsAsRest();
    refusal = [
      "/spec/catalog/visualizations/0/data/last: source: not allowed",
      "/spec/catalog/visualizations/0/spec/data: spec: a URL is not allowed",
    ];
    await proposeLine();
    expect(await screen.findByText(/a URL is not allowed/)).toBeVisible();
  });

  it("lists what readers start from, and what they are locked to without the toolbar (T-7491)", () => {
    const long = { ...WEEK, name: "no2-long", data: { ...WEEK.data, last: "P14D" } };
    const raw = { ...WEEK, name: "no2-raw", data: { source: "temporal", type: "T", attrs: ["no2"], last: "PT24H", controls: false } };
    mount(endpoint({ visualizations: [WEEK, raw, long] }), false);
    expect(screen.getByText("Readers start with Range: 14 days · Granularity: Daily · Method: Maximum")).toBeVisible();
    expect(screen.getByText("Readers start with Range: 7 days · Granularity: Daily · Method: Maximum")).toBeVisible();
    expect(screen.getByText("Readers see only Range: 24 hours · Granularity: Raw readings")).toBeVisible();
  });

  it("tells the editor what an unticked toolbar means for readers (T-7491)", async () => {
    mount(endpoint(), true);
    await userEvent.click(screen.getByText(V.add));
    await userEvent.click(screen.getByLabelText(V.controls));
    expect(screen.getByLabelText(V.fixed.range)).toBeInTheDocument();
    const box = screen.getByLabelText(V.controls);
    expect(box).toHaveAccessibleDescription(V.hint.locked);
    expect(box).not.toHaveAccessibleDescription(V.hint.controls);
  });

  it("empties the form once the chart is proposed, so it is not proposed twice (T-7491)", async () => {
    mount(endpoint(), true);
    doorsAsRest();
    await userEvent.click(screen.getByText(V.add));
    await userEvent.click(screen.getByLabelText(V.controls));
    await userEvent.selectOptions(screen.getByLabelText(V.fixed.range), "week");
    await userEvent.click(screen.getByText(V.add));
    await proposeLine();
    await waitFor(() => expect(screen.getByLabelText(V.name)).toHaveValue(""));
    expect(screen.getByLabelText(V.controls)).toBeChecked();
    expect(screen.getByLabelText(V.range)).toHaveValue("day");
    expect(screen.getByText(V.add).closest("details")).not.toHaveAttribute("open");
    expect(screen.getByText(V.awaiting).parentElement).toHaveTextContent("New");
    expect(screen.getByRole("button", { name: V.propose })).toHaveAttribute("aria-disabled", "true");
  });

  it("offers no toolbar defaults for a chart that does not read the history", async () => {
    mount(endpoint(), true);
    await userEvent.click(screen.getByText(V.add));
    await userEvent.selectOptions(screen.getByLabelText(V.template), "bar");
    expect(screen.queryByTestId("viz-temporal")).toBeNull();
  });

  it("refuses a name already taken and a spec that is not JSON before anything is sent", async () => {
    mount(endpoint({ visualizations: [BAR] }), true);
    await userEvent.click(screen.getByText(V.add));
    await userEvent.type(screen.getByLabelText(V.name), "no2-now");
    await userEvent.type(screen.getByLabelText(V.chartTitle), "T");
    await userEvent.type(screen.getByLabelText(V.description), "D");
    await userEvent.type(screen.getByLabelText(V.spec), "not json");
    expect(screen.getByRole("button", { name: V.propose })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
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
    await waitFor(() =>
      expect(sent.filter((r) => r.method === "PUT")).toHaveLength(2),
    );
    expect(JSON.parse(sent[1].body).spec.catalog).toBeUndefined();
  });
});

describe("visualization data", () => {
  it("reads the history where the Endpoint serves NGSI-LD, else the CSV, else nothing", () => {
    expect(defaultData("bar", "T", "v", ["odata", "csv"])).toEqual({
      source: "odata",
      entitySet: "T",
    });
    expect(defaultData("line", "T", "v", ["csv"])).toEqual({
      source: "csv",
      type: "T",
    });
    expect(defaultData("heatmap", "T", "v", ["ogc-features"])).toBeUndefined();
  });

  it("writes raw readings without an aggregation and a shown toolbar without the switch", () => {
    expect(
      defaultData("line", "T", "v", ["ngsi-ld"], { range: "month", granularity: "raw", method: "max", controls: true }),
    ).toEqual({ source: "temporal", type: "T", attrs: ["v"], last: "P31D" });
  });

  it("keeps the rest of the catalogue when the list empties", () => {
    const next = withVisualizations(
      endpoint({ license: "cc0", visualizations: [BAR] }),
      [],
    );
    expect((next.spec as { catalog: unknown }).catalog).toEqual({
      license: "cc0",
    });
  });
});
