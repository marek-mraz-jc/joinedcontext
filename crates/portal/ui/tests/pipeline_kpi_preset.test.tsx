/**
 * T-0633: Analyse (KPI) preset in the pipeline studio.
 * An indicator is generated from an endpoint query, folded into a KeyPerformanceIndicator,
 * tested through the test route against the live endpoint URL, and proposed.
 */
import { useState } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { BrandingProvider } from "../src/branding";
import en from "../src/locales/en.json";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import type { Manifest } from "../src/api/manifest";
import { sentTo } from "./requests";

function MockEditor({ value, onChange }: { value: string; onChange?: (value: string) => void }) {
  return (
    <textarea
      aria-label="YAML"
      value={value}
      onChange={(event) => onChange?.(event.target.value)}
    />
  );
}
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: MockEditor,
}));

const { PipelineStudio } = await import("../src/pages/pipelines/PipelineStudio");

const ENDPOINTS: Manifest[] = [
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "helsinki-all", namespace: "helsinki" },
    spec: { contextSpaceRef: "helsinki", slug: "abc123", audience: "public" },
  },
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "kpi-writer", namespace: "helsinki" },
    spec: { contextSpaceRef: "helsinki-kpi", slug: "kpi456" },
  },
];

/** The models the organization list answers: the type picker offers the endpoint's space's (T-2701). */
const MODELS = [
  { name: "bikes", project: "helsinki", space: "helsinki", version: "1.0.0", lifecycle: "published", classes: ["BikeHireDockingStation", "Road"] },
  { name: "kpi", project: "helsinki", space: "helsinki-kpi", version: "1.0.0", lifecycle: "published", classes: ["KeyPerformanceIndicator"] },
];

const KPI_TEST_ANSWER = {
  input: { events: 1, bytes: 10 },
  mapping: [{ id: "x", type: "KeyPerformanceIndicator", currentValue: { value: 12.5 } }],
  validation: [{ index: 0, ok: true, problems: [] }],
  errors: [],
};

/** One page of the source endpoint, as the gateway answers it to the person's session. */
const ENDPOINT_PAGE = [
  { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:h:1", type: "BikeHireDockingStation", availableBikeNumber: { type: "Property", value: 12.5 } },
];

/** The project's own space and model, for the attribute picker (T-3088). */
const BIKES_LINKML = [
  "id: https://hel.fi/models/bikes",
  "name: bikes",
  "classes:",
  "  BikeHireDockingStation:",
  "    slots: [availableBikeNumber, totalSlotNumber, name]",
  "slots:",
  "  availableBikeNumber: {range: integer}",
  "  totalSlotNumber: {range: integer}",
  "  name: {range: string}",
  "",
].join("\n");
const SPACES = [{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki" }, spec: { dataModelRef: { kind: "DataModel", name: "bikes" } } }];
const PROJECT_MODELS = [{ apiVersion: "joinedcontext.com/v1alpha1", kind: "DataModel", metadata: { name: "bikes", namespace: "helsinki" }, spec: { linkml: BIKES_LINKML } }];

function mockFetch(testResponse?: { status: number; body: unknown }, endpointStatus = 200, modelled = false) {
  const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const method = init?.method ?? (input instanceof Request ? input.method : "GET");
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );
    if (url.includes("/api/v1/projects/helsinki/datamodels/bikes/source")) {
      return Promise.resolve(new Response(BIKES_LINKML, { status: 200, headers: { "Content-Type": "text/yaml" } }));
    }
    if (url.includes("/api/v1/organization/datamodels")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: MODELS, smartDataModels: [] });
    }
    if (modelled && url.includes("/api/v1/projects/helsinki/spaces")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: SPACES });
    }
    if (modelled && url.includes("/api/v1/projects/helsinki/datamodels")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: PROJECT_MODELS });
    }
    if (url.includes("/api/endpoint/")) {
      return endpointStatus === 200
        ? json(ENDPOINT_PAGE)
        : json({ title: "Authentication Required", detail: "Authentication Required", status: endpointStatus }, endpointStatus);
    }
    if (url.includes("/pipelines/test") && method === "POST") {
      return json(testResponse?.body ?? {}, testResponse?.status ?? 200);
    }
    return json({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "List",
      items: [],
    });
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function Harness({
  initial,
  onChange,
  onVerdict,
  endpoints = ENDPOINTS,
}: {
  initial?: PipelineForm;
  onChange: (form: PipelineForm) => void;
  onVerdict?: (ok: boolean, bloblang: string) => void;
  endpoints?: Manifest[];
}) {
  const [draft, setDraft] = useState<PipelineForm | undefined>(initial);
  return (
    <PipelineStudio
      project="helsinki"
      draft={draft}
      onChange={(form) => {
        setDraft(form);
        onChange(form);
      }}
      dataSources={[]}
      endpoints={endpoints}
      toManifest={(form) => ({
        kind: "Pipeline",
        metadata: { name: form.name },
        spec: form,
      })}
      onVerdict={onVerdict}
    />
  );
}

function renderStudio(options?: {
  initial?: PipelineForm;
  onChange?: (form: PipelineForm) => void;
  onVerdict?: (ok: boolean, bloblang: string) => void;
  endpoints?: Manifest[];
}) {
  const onChange = options?.onChange ?? vi.fn();
  const onVerdict = options?.onVerdict ?? vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  // The installation's domain, which a KPI target URN names; without it no target is offered.
  client.setQueryData(["branding"], { orgDomain: "hel.fi" });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <BrandingProvider>
          <Harness
            initial={options?.initial}
            onChange={onChange}
            onVerdict={onVerdict}
            endpoints={options?.endpoints ?? ENDPOINTS}
          />
        </BrandingProvider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { onChange, onVerdict };
}

describe("PipelineStudio KPI preset", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("choosing preset kpi and endpoint helsinki-all calls onChange with a KPI form", async () => {
    mockFetch();
    const { onChange } = renderStudio();

    const presetSelect = screen.getByLabelText(en.pipelines.studio.preset.title);
    await userEvent.selectOptions(presetSelect, "kpi");

    const endpointSelect = await screen.findByLabelText(en.pipelines.studio.kpi.endpoint);
    await userEvent.selectOptions(endpointSelect, "helsinki-all");

    await waitFor(() => expect(onChange).toHaveBeenCalled());
    const lastForm = (onChange as ReturnType<typeof vi.fn>).mock.calls.at(-1)?.[0] as PipelineForm;
    expect(lastForm.source?.endpointRef).toBe(
      "helsinki-all",
    );
    expect(lastForm.output?.type).toBe("KeyPerformanceIndicator");
    expect(lastForm.targetEndpoint).toContain("kpi-writer");
    expect(lastForm.compute?.bloblang).toContain("availableBikeNumber");
    expect(lastForm.compute?.bloblang).toContain('"C62"');
    // The broker refuses a nanosecond observedAt, so the preset stamps whole seconds.
    expect(lastForm.compute?.bloblang).toContain('now().ts_format("2006-01-02T15:04:05Z")');
  });

  it("picks the counted type from the classes of the chosen endpoint's space, never a typed name", async () => {
    mockFetch();
    const { onChange } = renderStudio();
    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.studio.preset.title), "kpi");
    await userEvent.selectOptions(await screen.findByLabelText(en.pipelines.studio.kpi.endpoint), "helsinki-all");

    await userEvent.click(screen.getByRole("combobox", { name: en.pipelines.studio.kpi.type }));
    const list = await screen.findByRole("listbox", { name: en.picker.list });
    await waitFor(() =>
      expect(within(list).getAllByRole("option").map((o) => o.textContent)).toEqual([
        expect.stringContaining("BikeHireDockingStation"),
        expect.stringContaining("Road"),
      ]),
    );
    await userEvent.click(within(list).getByRole("option", { name: /Road/ }));
    await waitFor(() => {
      const form = (onChange as ReturnType<typeof vi.fn>).mock.calls.at(-1)?.[0] as PipelineForm;
      expect(form.compute?.bloblang).toContain("over Road");
    });
  });

  it("reads the endpoint's page with the person's session and tests on it inline", async () => {
    const fetchMock = mockFetch({ status: 200, body: KPI_TEST_ANSWER });
    const { onVerdict } = renderStudio();

    const presetSelect = screen.getByLabelText(en.pipelines.studio.preset.title);
    await userEvent.selectOptions(presetSelect, "kpi");
    const endpointSelect = await screen.findByLabelText(en.pipelines.studio.kpi.endpoint);
    await userEvent.selectOptions(endpointSelect, "helsinki-all");

    const testBtn = screen.getByTestId("studio-kpi-test");
    await userEvent.click(testBtn);

    await waitFor(() => expect(sentTo(fetchMock, "/pipelines/test")).toHaveLength(1));

    // The runner fetches a URL with no credential and the endpoint answered it 401 (T-3088):
    // the browser reads the page the reconciler will read, and the test carries it as text.
    const read = sentTo(fetchMock, "/api/endpoint/abc123/ngsi-ld/v1/entities");
    expect(read).toHaveLength(1);
    expect(read[0].path).toContain("type=BikeHireDockingStation");
    expect(read[0].path).toContain("limit=1000");
    const body = (await sentTo(fetchMock, "/pipelines/test")[0].json()) as {
      sample: { text?: string; url?: string; format: string };
      pipeline: { spec: PipelineForm };
    };
    expect(body.sample.url).toBeUndefined();
    expect(JSON.parse(body.sample.text ?? "")).toEqual(ENDPOINT_PAGE);
    expect(body.sample.format).toBe("json");

    const valueEl = await screen.findByTestId("studio-kpi-value");
    expect(valueEl).toHaveTextContent("12.5");
    expect(onVerdict).toHaveBeenCalledWith(true, expect.stringContaining("availableBikeNumber"));
  });

  it("offers the numbers the space's model lists as the attribute, not a text box", async () => {
    mockFetch(undefined, 200, true);
    const { onChange } = renderStudio();
    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.studio.preset.title), "kpi");
    await userEvent.selectOptions(await screen.findByLabelText(en.pipelines.studio.kpi.endpoint), "helsinki-all");
    // The model's numbers are offered in a picker, never a text box (T-4946).
    const attribute = screen.getByRole("combobox", { name: en.pipelines.studio.kpi.attribute });
    expect(attribute).toHaveAccessibleDescription(en.pipelines.studio.kpi.attributePick);
    await userEvent.click(attribute);
    expect(await screen.findByRole("option", { name: /totalSlotNumber/ })).toBeInTheDocument();
    const offered = screen.getAllByRole("option").map((option) => option.textContent ?? "");
    expect(offered.some((text) => text.includes("availableBikeNumber"))).toBe(true);
    expect(offered.some((text) => /^name/.test(text))).toBe(false);
    await userEvent.click(screen.getByRole("option", { name: /totalSlotNumber/ }));
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ source: expect.objectContaining({ query: expect.objectContaining({ attrs: ["totalSlotNumber"] }) }) }),
    );
  });

  it("says the endpoint's refusal in words and asks the runner nothing", async () => {
    const fetchMock = mockFetch({ status: 200, body: KPI_TEST_ANSWER }, 401);
    renderStudio();
    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.studio.preset.title), "kpi");
    await userEvent.selectOptions(await screen.findByLabelText(en.pipelines.studio.kpi.endpoint), "helsinki-all");
    await userEvent.click(screen.getByTestId("studio-kpi-test"));
    expect(await screen.findByRole("alert")).toHaveTextContent("Authentication Required");
    expect(sentTo(fetchMock, "/pipelines/test")).toHaveLength(0);
  });

  it("shows the trace's first error instead of nothing when the mapping fails", async () => {
    mockFetch({
      status: 200,
      body: {
        ...KPI_TEST_ANSWER,
        mapping: [],
        validation: [],
        errors: [{ stage: "mapping", line: 3, message: "expected number, got null" }],
      },
    });
    renderStudio();

    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.studio.preset.title), "kpi");
    await userEvent.selectOptions(
      await screen.findByLabelText(en.pipelines.studio.kpi.endpoint),
      "helsinki-all",
    );

    await userEvent.click(screen.getByTestId("studio-kpi-test"));

    expect(await screen.findByRole("alert")).toHaveTextContent("mapping: expected number, got null");
    expect(screen.queryByTestId("studio-kpi-value")).toBeNull();
  });

  it("shows an alert when the pipeline test endpoint fails", async () => {
    mockFetch({
      status: 500,
      body: { detail: "Failed to fetch from endpoint" },
    });
    renderStudio();

    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.studio.preset.title), "kpi");
    await userEvent.selectOptions(
      await screen.findByLabelText(en.pipelines.studio.kpi.endpoint),
      "helsinki-all",
    );

    await userEvent.click(screen.getByTestId("studio-kpi-test"));

    expect(await screen.findByRole("alert")).toHaveTextContent("Failed to fetch from endpoint");
  });

  it("says nothing was read when the sample is empty and shows no value", async () => {
    mockFetch({
      status: 200,
      body: { input: { events: 0, bytes: 0 }, mapping: [], validation: [], errors: [] },
    });
    const { onVerdict } = renderStudio();

    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.studio.preset.title), "kpi");
    await userEvent.selectOptions(
      await screen.findByLabelText(en.pipelines.studio.kpi.endpoint),
      "helsinki-all",
    );

    await userEvent.click(screen.getByTestId("studio-kpi-test"));

    expect(await screen.findByRole("alert")).toHaveTextContent(en.pipelines.studio.kpi.nothingRead);
    expect(screen.queryByTestId("studio-kpi-value")).toBeNull();
    expect(onVerdict).toHaveBeenCalledWith(false, expect.any(String));
  });

  it("tests the draft's own mapping, so an ok trace opens Propose for what is proposed", async () => {
    // The assistant (or the YAML view) wrote the mapping; a verdict on a regenerated one would
    // belong to a text nobody proposes, and Propose would stay shut (PL-49, T-0911).
    const written = 'root = {"id": "urn:ngsi-ld:KeyPerformanceIndicator:x", "hand": "written"}';
    const fetchMock = mockFetch({ status: 200, body: KPI_TEST_ANSWER });
    const { onVerdict } = renderStudio({
      initial: {
        name: "bikes-available-avg",
        class: "auto",
        period: "15m",
        source: {
          endpointRef: "helsinki-all",
          query: { type: "BikeHireDockingStation", attrs: ["availableBikeNumber"] },
        },
        compute: { kind: "bloblang", bloblang: written },
        output: { type: "KeyPerformanceIndicator", mode: "upsert" },
        targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:helsinki-kpi:kpi-writer",
      } as PipelineForm,
    });

    await userEvent.click(await screen.findByTestId("studio-kpi-test"));

    await waitFor(() => expect(onVerdict).toHaveBeenCalledWith(true, written));
    const body = (await sentTo(fetchMock, "/pipelines/test")[0].json()) as {
      pipeline: { spec: PipelineForm };
    };
    expect(body.pipeline.spec.compute?.bloblang).toBe(written);
    expect(await screen.findByTestId("studio-kpi-value")).toHaveTextContent("12.5");
  });

  it("updates Bloblang and calls onChange when attribute or aggregate change", async () => {
    mockFetch();
    const { onChange } = renderStudio();

    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.studio.preset.title), "kpi");
    await userEvent.selectOptions(
      await screen.findByLabelText(en.pipelines.studio.kpi.endpoint),
      "helsinki-all",
    );

    await userEvent.click(screen.getByRole("combobox", { name: en.pipelines.studio.kpi.attribute }));
    await userEvent.click(await screen.findByRole("option", { name: /totalSlotNumber/ }));

    await waitFor(() => {
      const form = (onChange as ReturnType<typeof vi.fn>).mock.calls.at(-1)?.[0] as PipelineForm;
      expect(form.compute?.bloblang).toContain("totalSlotNumber");
    });
  });
});
