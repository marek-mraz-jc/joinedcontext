/** T-0497: a Pipeline from a form or its YAML, proposed through the change flow (PL-04, PL-31, PL-33, PL-39, UI-01, AP-13). */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { expectNoRawKeys, expectNoViolations, expectTabOrder, expectOpen } from "./checks";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { rememberPrefill } from "../src/assistant/state";
import { greenVerdict, isCheck } from "./verdict";
import { findFormPage, queryFormPage } from "./formPage";

// Monaco draws on a canvas and starts a worker, neither of which exists in jsdom. The stand-in
// is a textarea with the same contract, so what the test exercises is the dialog's own work:
// the YAML it writes, the manifest it reads back and the validation between them.
function MockEditor({ value, onChange }: { value: string; onChange?: (value: string) => void }) {
  return (
    <textarea
      aria-label="YAML"
      value={value}
      onChange={(event) => onChange?.(event.target.value)}
    />
  );
}

vi.mock("../src/pages/models/MonacoSourceView", () => ({ default: MockEditor }));

// One user per case, without the zero-delay timer userEvent waits between keys by default: the
// typed names and mappings are many keystrokes, each a render of the whole form (T-5520).
let user: UserEvent;
beforeEach(() => {
  user = userEvent.setup({ delay: null });
});

const { App } = await import("../src/App");
const { canvasLoss, completeOutput, endpointUrn, expiryWindow, firstShape, fromManifest, toEnvelope, toForm } = await import(
  "../src/pages/pipelines/PipelineEditor"
);
const { aggregateBloblang, attributesOf, sourceKindOf } = await import(
  "../src/pages/pipelines/PipelineStudio"
);

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  // A plain editor, not an approver: the button is for everyone signed in.
  roles: ["portal-editor"],
};

const BRANDING = {
  instanceName: "joinedcontext",
  shortName: "joinedcontext",
  city: "Banská Bystrica",
  organisation: "Mesto Banská Bystrica",
  orgDomain: "banskabystrica.sk",
  domain: "portal.banskabystrica.sk",
  contactEmail: "data@banskabystrica.sk",
  licenseDefault: "CC-BY-4.0",
  logo: "",
  favicon: "",
  colours: {
    primary: "#1d4ed8",
    secondary: "#0f766e",
    accent: "#f59e0b",
    background: "#ffffff",
    text: "#0f172a",
  },
  fonts: { heading: "system-ui, sans-serif", body: "system-ui, sans-serif" },
  languages: { default: "en", offered: ["en"] },
  primaryForeground: "#ffffff",
};

const EXISTING: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "aq-mqtt-ingest", namespace: "banskabystrica" },
  spec: {
    class: "resident",
    enabled: false,
    source: { dataSourceRef: { kind: "DataSource", name: "mqtt-mesto" } },
    compute: { kind: "bloblang" },
    targetEndpoint: "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air",
    secretRefs: [{ name: "mqtt-credentials", key: "password", envVar: "MQTT_PASSWORD" }],
  },
  status: {
    phase: "Live",
    sourceUrl:
      "https://git.example.sk/bb/org/src/branch/main/projects/banskabystrica/pipelines/aq-mqtt-ingest/pipeline.yaml",
  },
};

const list = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});

const DATASOURCES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: { name: "mqtt-mesto", namespace: "banskabystrica" },
    spec: { type: "mqtt", mqtt: { urls: ["tls://mqtt.banskabystrica.sk:8883"], topics: ["aq/#"] } },
  },
]);

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "public-air", namespace: "banskabystrica" },
    spec: { contextSpaceRef: "ovzdusie", slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa", audience: "public" },
  },
]);

const SPACES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "ovzdusie", namespace: "banskabystrica", title: { en: "Air quality" } },
    spec: { dataModelRef: "bb-air-quality" },
  },
]);

const AIR_MODEL = [
  "id: https://banskabystrica.sk/models/air",
  "name: bb-air-quality",
  "classes:",
  "  AirQualityObserved:",
  "    slots: [id, pm10, pm25, refDistrict]",
  "  AirQualityStation:",
  "    slots: [id, name]",
  "slots:",
  "  id: {}",
  "  pm10: { range: float }",
  "  pm25: { range: float }",
  "  refDistrict: { range: string }",
  "  name: { range: string }",
  "",
].join("\n");

const MODELS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "bb-air-quality", namespace: "banskabystrica" },
    spec: { linkml: AIR_MODEL, version: "2.1.0", classes: ["AirQualityObserved"] },
  },
]);

const SAMPLE = [
  { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:radvan-01", type: "AirQualityObserved", pm10: 12, pm25: 4 },
  { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:radvan-02", type: "AirQualityObserved", pm10: 9, pm25: 3 },
  { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:sasova-01", type: "AirQualityObserved", pm10: 30, pm25: 11 },
];

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
};

/** The `bento.yaml` the editor loads (T-4818): its text, the status the read fails with, or none. */
function renderPipelines(pipelines: Manifest[] = [EXISTING], bento?: string | number) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    // The client sends Requests; the pipeline test posts a plain URL string.
    const url = new URL(typeof input === "string" ? input : request.url, "http://localhost");
    const path = url.pathname;
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
    if (path.endsWith("/branding")) {
      return json(BRANDING);
    }
    if (path.endsWith("/pipelines/test")) {
      return json(TRACE);
    }
    // The check is a dry run: it answers a verdict and writes nothing (AG-62, PF-57).
    if (input !== null && typeof input !== "string" && isCheck(request, url)) {
      return greenVerdict(request).then((body) => json(body));
    }
    if (request.method !== "GET") {
      return json(CHANGE, 202);
    }
    if (path.endsWith("/bento")) {
      return Promise.resolve(
        typeof bento === "string"
          ? new Response(bento, { status: 200, headers: { "Content-Type": "text/yaml" } })
          : new Response("{}", { status: bento ?? 404, headers: { "Content-Type": "application/json" } }),
      );
    }
    if (path.startsWith("/api/endpoint/")) {
      return json(SAMPLE);
    }
    if (path.endsWith("/pipelines")) {
      return json(list(pipelines));
    }
    if (path.endsWith("/spaces")) {
      return json(SPACES);
    }
    if (path.endsWith("/datamodels")) {
      return json(MODELS);
    }
    if (path.endsWith("/datasources")) {
      return json(DATASOURCES);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
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

/** A green trace for the studio's test: one entity out, nothing to report (PL-43). */
const TRACE = {
  input: { events: 1, bytes: 120 },
  mapping: [{ id: "urn:ngsi-ld:AirQualityObservedAggregate:banskabystrica.sk:ovzdusie:sum", type: "AirQualityObservedAggregate" }],
  validation: [{ index: 0, ok: true, problems: [] }],
  errors: [],
};

function writes(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    // The access panel's dry-run check and the pipeline test are POSTs that write nothing (T-0529, PL-43).
    .filter(
      (request) =>
        (request.method === "POST" || request.method === "PUT") &&
        !request.url.endsWith("/access/check") &&
        !request.url.endsWith("/access/v1/evaluations") &&
        !request.url.endsWith("/pipelines/test") &&
        // The workbench's steps sample, try and check; they write nothing either (PL-63).
        !/\/ops\/jc_pipeline_(sample_source|try_mapping|validate)$/.test(request.url) &&
        // The form's own draft, shared with the other windows; it writes nothing to Git (AG-61).
        !request.url.includes("/drafts/") &&
        // The Check's dry run answers a verdict and writes nothing either (AG-62).
        !new URL(request.url).searchParams.has("dryRun"),
    );
}

/**
 * A part of the studio or the workbench, by the id its heading carries (`studio-entities`,
 * `workbench-target-title`). The dialog holds about a thousand elements, and a label or role
 * query over all of them costs seconds in jsdom; over its own part, milliseconds (T-5520).
 */
function section(scope: HTMLElement, labelledBy: string): HTMLElement {
  const found = scope.querySelector<HTMLElement>(`section[aria-labelledby="${labelledBy}"]`);
  if (!found) {
    throw new Error(`no section labelled by ${labelledBy}`);
  }
  return found;
}

/**
 * The control a visible `<label>` names, found by reading the labels alone: getByLabelText
 * weighs every element of the dialog for a label, seconds per lookup (T-5520).
 */
function labelled(scope: HTMLElement, text: string | RegExp): HTMLElement {
  const label = within(scope).getByText(text, { selector: "label" }) as HTMLLabelElement;
  const control = label.htmlFor ? document.getElementById(label.htmlFor) : label.control;
  if (!control) {
    throw new Error(`the label ${String(text)} names no control`);
  }
  return control;
}

const targetPick = (dialog: HTMLElement) =>
  within(section(dialog, "workbench-target-title")).getByLabelText(en.pipelines.field.targetEndpoint);

async function openNew() {
  const add = await screen.findByRole("button", { name: en.pipelines.add });
  // The guard refuses while the rights are read (T-5833).
  await waitFor(() => expect(add).not.toHaveAttribute("aria-disabled"));
  await user.click(add);
  const dialog = await findFormPage();
  // The selects are filled from the project's lists once they arrive: the target is the
  // workbench's last step, the source its first (T-2754, T-2709).
  const target = targetPick(dialog);
  await within(target).findByRole("option", { name: "public-air" });
  return dialog;
}

/** The source is picked where a person picks it: the studio above the form (T-2754). */
async function pickDataSource(dialog: HTMLElement, name: string) {
  const studio = within(dialog).getByTestId("pipeline-studio");
  await user.selectOptions(within(section(studio, "studio-source")).getByLabelText(en.pipelines.studio.sourceKind), "datasource");
  await user.selectOptions(within(section(studio, "studio-source")).getByLabelText(en.pipelines.field.dataSource), name);
}

/**
 * The form's footer, where Check and Propose sit. A role query over the whole dialog computes the
 * accessible name and visibility of every control in it, seconds per lookup in jsdom (T-5520).
 */
const footer = (dialog: HTMLElement) => within(dialog).getByTestId("form-footer");
const yamlTab = (dialog: HTMLElement) => within(dialog).getByRole("tab", { name: en.form.view.yaml });
const formTab = (dialog: HTMLElement) => within(dialog).getByRole("tab", { name: en.form.view.form });
// Awaited: the editor is loaded lazily, so it is behind a Suspense boundary for a moment.
const editor = (dialog: HTMLElement) =>
  within(dialog).findByRole("textbox", { name: "YAML" }) as Promise<HTMLTextAreaElement>;

async function replaceYaml(dialog: HTMLElement, text: string) {
  const area = await editor(dialog);
  await user.clear(area);
  // `paste`, not `type`: userEvent would take `{` and `[` in the text as key descriptors.
  await user.click(area);
  await user.paste(text);
}

describe("pipeline editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/pipelines");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("completes a typed output with the upsert mode and leaves a chosen mode alone", () => {
    const typed = { class: "auto", output: { type: "Vehicle", mode: "" } } as PipelineForm;
    expect(completeOutput(typed)?.output).toEqual({ type: "Vehicle", mode: "upsert" });
    const chosen = { class: "auto", output: { type: "Vehicle", mode: "update-attrs" } } as PipelineForm;
    expect(completeOutput(chosen)?.output?.mode).toBe("update-attrs");
    expect(completeOutput({ class: "auto" } as PipelineForm)?.output).toBeUndefined();
    expect(completeOutput(undefined)).toBeUndefined();
  });

  it("maps a manifest to the form and back without losing a field", () => {
    const form = toForm(EXISTING);
    expect(form.source?.dataSourceRef).toBe("mqtt-mesto");
    expect(form).not.toHaveProperty("enabled");

    const envelope = toEnvelope("banskabystrica", form, EXISTING);
    expect(envelope.metadata).toEqual({ name: "aq-mqtt-ingest", namespace: "banskabystrica" });
    // Written at v1alpha2 (PL-54), with nothing lost: read back in the first shape it is the
    // manifest it came from.
    expect(envelope.apiVersion).toBe("joinedcontext.com/v1alpha2");
    expect(envelope.spec).not.toHaveProperty("source");
    expect(firstShape(envelope.spec)).toEqual(EXISTING.spec);
    // What the YAML view shows is what the form reads back.
    expect(fromManifest(envelope)).toEqual(form);
  });

  it("builds the target URN from the branding's domain, the endpoint's space and its name", () => {
    const endpoint = ENDPOINTS.items[0] as Manifest;
    expect(endpointUrn("banskabystrica.sk", endpoint)).toBe(
      "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air",
    );
    expect(endpointUrn("", endpoint)).toBeUndefined();
  });

  /// T-1135: what the form has no control for is what an edit must not take away (T-0885).
  it("keeps the fields the form does not show, and the ones it does", () => {
    const rich = {
      ...EXISTING,
      spec: {
        ...EXISTING.spec,
        enabled: false,
        policyRef: { kind: "Policy", name: "ingest-write" },
        publish: { ckanInstanceRef: "opendata" },
      },
    } as Manifest;

    const envelope = toEnvelope("banskabystrica", toForm(rich), rich);
    const spec = envelope.spec as Record<string, unknown>;
    // The pause button owns `enabled`, and the form has no control for the other two.
    expect(spec.enabled).toBe(false);
    expect(spec.policyRef).toEqual({ kind: "Policy", name: "ingest-write" });
    expect(spec.publish).toEqual({ ckanInstanceRef: "opendata" });
  });

  /// PL-64, PL-65: expiry round-trips, an emptied one is removed, and the window reads as words.
  it("writes an expiry the form holds, drops an emptied one, and says the window in words", () => {
    const expiring = {
      ...EXISTING,
      spec: { ...EXISTING.spec, expiry: { after: "14d", types: ["Vehicle"] } },
    } as Manifest;
    const form = toForm(expiring);
    expect(form.expiry).toEqual({ after: "14d", types: ["Vehicle"] });
    const written = toEnvelope("banskabystrica", form, expiring).spec as Record<string, unknown>;
    expect(written.expiry).toEqual({ after: "14d", types: ["Vehicle"] });

    // Clearing both fields turns it off: absent, never an empty object the manifest refuses.
    for (const emptied of [{}, { after: "", types: [] }, { types: [] }]) {
      const off = toEnvelope("banskabystrica", { ...form, expiry: emptied }, expiring);
      expect(off.spec).not.toHaveProperty("expiry");
    }

    expect(expiryWindow("14d", "en")).toBe("14 days");
    expect(expiryWindow("1h", "en")).toBe("1 hour");
    expect(expiryWindow("2d", "de")).toBe("2 Tage");
    expect(expiryWindow("weekly", "en")).toBe("weekly");
  });

  it("reads a manifest back into the form, and refuses one with no name", () => {
    const bare = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: { name: "bare", namespace: "banskabystrica" },
    };
    const form = fromManifest(bare);
    expect(form.name).toBe("bare");
    expect(form.source).toBeUndefined();
    expect(form.compute).toBeUndefined();

    // A document that names no resource is not a manifest, and the YAML view says so rather
    // than proposing something addressed to nothing (MF-02).
    expect(() => fromManifest({ kind: "Pipeline", spec: {} })).toThrow(/metadata.name/);
    expect(() => fromManifest({})).toThrow(/metadata.name/);
  });

  it("builds no target URN when the endpoint names no space", () => {
    const spaceless = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "public-air", namespace: "banskabystrica" },
      spec: { slug: "x" },
    } as Manifest;
    expect(endpointUrn("banskabystrica.sk", spaceless)).toBeUndefined();
  });

  it("round-trips the form through the YAML view and keeps the manifest", async () => {
    renderPipelines();
    const dialog = await openNew();

    await user.type(labelled(dialog, /^Name/), "aq-derived");
    await pickDataSource(dialog, "mqtt-mesto");
    await user.selectOptions(
      targetPick(dialog),
      "public-air",
    );

    await user.click(yamlTab(dialog));
    const shown = parseYaml((await editor(dialog)).value) as ReturnType<typeof toEnvelope>;
    expect(shown.kind).toBe("Pipeline");
    expect(shown.metadata.name).toBe("aq-derived");
    expect(shown.apiVersion).toBe("joinedcontext.com/v1alpha2");
    expect(shown.spec).toEqual({
      class: "auto",
      sources: [{ dataSourceRef: { kind: "DataSource", name: "mqtt-mesto" } }],
      outputs: [{ targetEndpoint: "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air" }],
    });

    // Edited as text, read back into the form.
    await replaceYaml(
      dialog,
      (await editor(dialog)).value.replace("class: auto", "class: auto\n  period: 15s"),
    );
    await user.click(formTab(dialog));
    expect(labelled(dialog, /^Period/)).toHaveValue("15s");
    expect(labelled(dialog, /^Name/)).toHaveValue("aq-derived");

    await user.click(yamlTab(dialog));
    expect(parseYaml((await editor(dialog)).value)).toEqual({
      ...shown,
      spec: { ...shown.spec, period: "15s" },
    });
  });

  it("keeps a YAML that does not parse in the editor and sends nothing", async () => {
    const fetchMock = renderPipelines();
    const dialog = await openNew();

    await user.click(yamlTab(dialog));
    await replaceYaml(dialog, "metadata: [unclosed");
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await user.click(within(footer(dialog)).getByRole("button", { name: en.form.check }));
    await waitFor(() =>
      expectOpen(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose })),
    );
    await user.click(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(/Correct the YAML/);
    expect(writes(fetchMock)).toHaveLength(0);

    // The form tab is refused too: the text stays where the problem is.
    await user.click(formTab(dialog));
    expect(await editor(dialog)).toHaveValue("metadata: [unclosed");
  });

  it("refuses a manifest the schema refuses, from the YAML view as from the form", async () => {
    const fetchMock = renderPipelines();
    const dialog = await openNew();

    await user.click(yamlTab(dialog));
    // A scheduled pipeline without a schedule, and a source naming both inputs (PL-39).
    await replaceYaml(
      dialog,
      [
        "apiVersion: joinedcontext.com/v1alpha1",
        "kind: Pipeline",
        "metadata:",
        "  name: nightly",
        "spec:",
        "  class: scheduled",
        "  source:",
        "    dataSourceRef: mqtt-mesto",
        "    endpointRef: public-air",
        "  targetEndpoint: urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air",
      ].join("\n"),
    );
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await user.click(within(footer(dialog)).getByRole("button", { name: en.form.check }));
    await waitFor(() =>
      expectOpen(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose })),
    );
    await user.click(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(en.form.schemaErrors);
    expect(alert).toHaveTextContent(en.form.required);
    expect(alert).toHaveTextContent(en.form.exclusive);
    expect(writes(fetchMock)).toHaveLength(0);
  });

  it("creates a pipeline with a POST carrying the manifest, and shows the change", async () => {
    const fetchMock = renderPipelines();
    const dialog = await openNew();

    await user.type(labelled(dialog, /^Name/), "aq-derived");
    await user.selectOptions(
      labelled(dialog, /^Execution/),
      en.pipelines.field.classChoice.resident,
    );
    await pickDataSource(dialog, "mqtt-mesto");
    await user.selectOptions(
      targetPick(dialog),
      "public-air",
    );
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await user.click(within(footer(dialog)).getByRole("button", { name: en.form.check }));
    await waitFor(() =>
      expectOpen(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose })),
    );
    await user.click(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("POST");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/banskabystrica/pipelines");
    await expect(request.clone().json()).resolves.toEqual({
      apiVersion: "joinedcontext.com/v1alpha2",
      kind: "Pipeline",
      metadata: { name: "aq-derived", namespace: "banskabystrica" },
      spec: {
        class: "resident",
        sources: [{ dataSourceRef: { kind: "DataSource", name: "mqtt-mesto" } }],
        outputs: [{ targetEndpoint: "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air" }],
      },
      // The form edits a Portal draft, and the proposal names the draft it was taken from,
      // so the verdict stored on that draft is the one the operation reads (AG-61, AG-62).
      draft: { kind: "Pipeline", name: "aq-derived" },
    });
    expect(await screen.findByText(/chg-77aa11bb/)).toBeInTheDocument();
    expect(queryFormPage()).not.toBeInTheDocument();
  });

  it("edits the first source, step and output of a merged pipeline and keeps the rest (PL-54)", () => {
    const merged: Manifest = {
      apiVersion: "joinedcontext.com/v1alpha2",
      kind: "Pipeline",
      metadata: { name: "bikes-merged", namespace: "banskabystrica" },
      spec: {
        class: "resident",
        sources: [
          { dataSourceRef: { kind: "DataSource", name: "mqtt-mesto" } },
          { dataSourceRef: { kind: "DataSource", name: "second-feed" } },
        ],
        steps: [
          { processor: { log: { message: "in" } } },
          { kind: "bloblang", bloblang: "root = this" },
          { processor: { dedupe: { cache: "c" } } },
        ],
        outputs: [
          { targetEndpoint: "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air", mode: "upsert" },
          { targetEndpoint: "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:kpi" },
        ],
      },
    };
    const form = toForm(merged);
    expect(form.source?.dataSourceRef).toBe("mqtt-mesto");
    expect(form.compute).toEqual({ kind: "bloblang", bloblang: "root = this" });
    expect(form.targetEndpoint).toBe("urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air");
    expect(form.output).toEqual({ mode: "upsert" });

    const edited = toEnvelope(
      "banskabystrica",
      { ...form, compute: { kind: "bloblang", bloblang: "root = this.changed" } },
      merged,
    );
    const spec = edited.spec as Record<string, unknown[]>;
    expect(edited.apiVersion).toBe("joinedcontext.com/v1alpha2");
    expect(spec.sources).toHaveLength(2);
    expect(spec.sources[1]).toEqual({ dataSourceRef: { kind: "DataSource", name: "second-feed" } });
    // The compute step changes in its own place, between the two processors.
    expect(spec.steps).toEqual([
      { processor: { log: { message: "in" } } },
      { kind: "bloblang", bloblang: "root = this.changed" },
      { processor: { dedupe: { cache: "c" } } },
    ]);
    expect(spec.outputs).toHaveLength(2);
    expect(edited.spec).not.toHaveProperty("targetEndpoint");

    // Removing the compute removes that step and no other.
    const without = toEnvelope("banskabystrica", { ...form, compute: undefined }, merged);
    expect((without.spec as Record<string, unknown[]>).steps).toEqual([
      { processor: { log: { message: "in" } } },
      { processor: { dedupe: { cache: "c" } } },
    ]);
  });

  it("names what the canvas holds that a pipeline without a first source or target cannot carry (T-3969)", () => {
    const target = "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air";
    const step = { step: { processor: { log: { message: "in" } } } };
    const extraSource = { dataSourceRef: "second-feed" } as NonNullable<PipelineForm["moreSources"]>[number];
    const extraOutput = { targetEndpoint: target } as NonNullable<PipelineForm["moreOutputs"]>[number];
    // An extra source, a processor step, an extra output: none fits v1alpha1, so each is named.
    for (const drawn of [{ moreSources: [extraSource] }, { processors: [step] }, { moreOutputs: [extraOutput] }]) {
      const form = { class: "auto", targetEndpoint: target, ...drawn } as PipelineForm;
      expect(canvasLoss(form)).toEqual({ key: "noSource", field: "source" });
      expect(toEnvelope("banskabystrica", form).apiVersion).toBe("joinedcontext.com/v1alpha1");
    }
    const sourced = { class: "auto", source: { dataSourceRef: "mqtt-mesto" } } as PipelineForm;
    expect(canvasLoss({ ...sourced, moreOutputs: [extraOutput] })).toEqual({ key: "noTarget", field: "targetEndpoint" });
    expect(canvasLoss({ ...sourced, processors: [step] })).toEqual({ key: "noTarget", field: "targetEndpoint" });
    // With both, everything drawn reaches the manifest, and nothing is refused.
    const whole = { ...sourced, targetEndpoint: target, processors: [step], moreSources: [extraSource], moreOutputs: [extraOutput] };
    expect(canvasLoss(whole)).toBeUndefined();
    const spec = toEnvelope("banskabystrica", whole).spec as Record<string, unknown[]>;
    expect(spec.sources).toHaveLength(2);
    expect(spec.steps).toEqual([{ processor: { log: { message: "in" } } }]);
    expect(spec.outputs).toHaveLength(2);
    // A pipeline whose input lives in bento.yaml draws nothing more, and stays as it is.
    expect(canvasLoss({ class: "auto", targetEndpoint: target } as PipelineForm)).toBeUndefined();
    expect(canvasLoss(undefined)).toBeUndefined();
  });

  it("keeps a pipeline whose input lives in bento.yaml at v1alpha1, which has a place for it", () => {
    const form = { ...toForm(EXISTING), source: undefined };
    const envelope = toEnvelope("banskabystrica", form);
    expect(envelope.apiVersion).toBe("joinedcontext.com/v1alpha1");
    expect(envelope.spec).toHaveProperty("targetEndpoint");
    expect(envelope.spec).not.toHaveProperty("outputs");
  });

  it("round-trips the inline Bloblang of a bloblang step through the manifest", () => {
    const mapping = 'root = this\nroot.status = { "type": "Property", "value": "ok" }\n';
    const form = { ...toForm(EXISTING), compute: { kind: "bloblang", bloblang: mapping } };
    const envelope = toEnvelope("banskabystrica", form, EXISTING);
    expect(envelope.spec.steps).toEqual([{ kind: "bloblang", bloblang: mapping }]);
    expect(fromManifest(envelope).compute?.bloblang).toBe(mapping);
    // An empty mapping is no mapping: the field is left out, and bento.yaml keeps it (PL-41).
    const blank = toEnvelope("banskabystrica", { ...form, compute: { kind: "bloblang", bloblang: "" } });
    expect(blank.spec.steps).toEqual([{ kind: "bloblang" }]);
  });

  it("shows a paused pipeline as paused, with a Resume rather than a Pause", async () => {
    renderPipelines();
    const row = (await screen.findByText("aq-mqtt-ingest")).closest("tr") as HTMLElement;
    expect(within(row).getByText(en.phase.paused)).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: en.pipelines.resume })).toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: en.pipelines.pause })).not.toBeInTheDocument();
  });

  it("edits the Bloblang of a bloblang step in the form, and asks wasm for its module", async () => {
    renderPipelines();
    const dialog = await openNew();

    expect(within(dialog).queryByText(en.pipelines.bloblangHint)).not.toBeInTheDocument();
    await user.selectOptions(labelled(dialog, /^Kind/), en.pipelines.field.computeChoice.bloblang);
    expect(within(dialog).getByText(en.pipelines.bloblangHint)).toBeInTheDocument();
    // A textarea, not a one-line input: a mapping is several lines of Bloblang (PL-41).
    expect(labelled(dialog, /^Bloblang mapping/).tagName).toBe("TEXTAREA");

    await user.selectOptions(labelled(dialog, /^Kind/), en.pipelines.field.computeChoice.wasm);
    expect(within(dialog).queryByText(en.pipelines.bloblangHint)).not.toBeInTheDocument();
    await user.type(labelled(dialog, /^Name/), "aq-index");
    // wasm needs module and function (PL-33): Check names both required fields on the compute
    // group and sends nothing, so no verdict opens Propose (AG-62, T-0779, T-2634).
    await user.click(within(footer(dialog)).getByRole("button", { name: en.form.check }));
    const alerts = await within(dialog).findAllByRole("alert");
    expect(alerts.filter((alert) => alert.textContent?.includes(en.form.required)).length)
      .toBeGreaterThanOrEqual(2);
    expect(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

it("tells a feed from a space and reads the attributes of a class from an inline model", () => {
    expect(sourceKindOf(undefined)).toBe("none");
    expect(sourceKindOf({ source: { dataSourceRef: "mqtt-mesto" } })).toBe("datasource");
    expect(sourceKindOf({ source: { endpointRef: "public-air" } })).toBe("space");
    const model = MODELS.items[0] as Manifest;
    expect(attributesOf(model, "AirQualityObserved")).toEqual(["pm10", "pm25", "refDistrict"]);
    expect(attributesOf(model, "Nope")).toEqual([]);
    expect(attributesOf(undefined, "AirQualityObserved")).toEqual([]);
  });

  it("writes an aggregate as one derived entity in an array, with provenance", () => {
    const sum = aggregateBloblang("sum", {
      type: "AirQualityObserved",
      attribute: "pm10",
      space: "ovzdusie",
      outputType: "AirQualityObservedAggregate",
    });
    expect(sum).toContain('"pm10Sum": { "type": "Property", "value": this.map_each(e -> e.pm10.value.number().catch(0)).sum()');
    expect(sum).toContain('"derivedFrom": { "type": "Property", "value": this.map_each(e -> e.id) }');
    expect(sum).toContain('"type": "AirQualityObservedAggregate"');
    expect(sum.trim().startsWith("#")).toBe(true);
    const count = aggregateBloblang("count", {
      type: "AirQualityObserved",
      attribute: "pm10",
      space: "ovzdusie",
      outputType: "AirQualityObservedAggregate",
    });
    expect(count).toContain('"pm10Count": { "type": "Property", "value": this.length()');
    const average = aggregateBloblang("average", {
      type: "AirQualityObserved",
      attribute: "pm25",
      space: "ovzdusie",
      outputType: "X",
    });
    expect(average).toContain("if this.length() == 0 { 0 } else {");
  });

  it("guides source, entities, a ticked sample and a sum into one proposed manifest (UI-32)", async () => {
    const fetchMock = renderPipelines();
    const dialog = await openNew();
    const studio = within(dialog).getByTestId("pipeline-studio");

    await user.selectOptions(within(section(studio, "studio-source")).getByLabelText(en.pipelines.studio.sourceKind), "space");
    await user.selectOptions(within(section(studio, "studio-source")).getByLabelText(en.pipelines.studio.space), "ovzdusie");
    // The read endpoint of the space is picked for the author, and stays a choice.
    expect(within(section(studio, "studio-source")).getByLabelText(en.pipelines.studio.readThrough)).toHaveValue("public-air");
    await user.selectOptions(within(section(studio, "studio-entities")).getByLabelText(en.entities.type), "AirQualityObserved");
    // The class's attributes come from the inline model.
    await user.click(within(section(studio, "studio-entities")).getByLabelText("pm10"));

    await user.click(within(section(studio, "studio-entities")).getByRole("button", { name: en.pipelines.studio.loadSample }));
    const first = await within(section(studio, "studio-entities")).findByLabelText(SAMPLE[0].id);
    await user.click(first);
    await user.click(within(section(studio, "studio-entities")).getByLabelText(SAMPLE[2].id));
    expect(within(studio).getByText(/2 entities ticked/)).toBeInTheDocument();
    const gatewayCalls = fetchMock.mock.calls
      .map((call) => new URL((call[0] as Request).url))
      // The sample read; the access panel's own calls (T-0529) are beside it.
      .filter((url) => url.pathname.startsWith("/api/endpoint/") && url.pathname.includes("/ngsi-ld/"))
      // The workbench reads the same page as the pipeline will, normalized, for its test (T-3088).
      .filter((url) => url.searchParams.get("options") === "keyValues");
    expect(gatewayCalls).toHaveLength(1);
    expect(gatewayCalls[0].pathname).toBe("/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/ngsi-ld/v1/entities");
    expect(gatewayCalls[0].searchParams.get("options")).toBe("keyValues");
    expect(gatewayCalls[0].searchParams.get("attrs")).toBe("pm10");

    await user.click(within(section(studio, "studio-process")).getByRole("button", { name: en.pipelines.studio.aggregate.sum }));

    // A Bloblang mapping proposes only after a green test (PL-49): the studio offers the
    // endpoint's own page as the sample, the way the reconciler will read it.
    await user.click(within(section(studio, "studio-test")).getByRole("button", { name: en.pipelines.test.useUrl }));
    await user.click(within(section(studio, "studio-test")).getByRole("button", { name: en.pipelines.test.run }));
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some((call) => String((call[0] as Request).url ?? call[0]).endsWith("/pipelines/test")),
      ).toBe(true),
    );

    await user.type(labelled(dialog, /^Name/), "pm10-sum");
    await user.selectOptions(
      targetPick(dialog),
      "public-air",
    );
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await user.click(within(footer(dialog)).getByRole("button", { name: en.form.check }));
    const propose = within(footer(dialog)).getByRole("button", { name: en.pipelines.propose });
    await waitFor(() => expect(propose).toBeEnabled());
    await user.click(propose);

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const body = (await writes(fetchMock)[0].clone().json()) as { spec: Record<string, unknown> };
    expect(firstShape(body.spec)).toMatchObject({
      class: "auto",
      period: "1h",
      source: {
        endpointRef: { kind: "Endpoint", name: "public-air" },
        query: { type: "AirQualityObserved", attrs: ["pm10"], ids: [SAMPLE[0].id, SAMPLE[2].id] },
      },
      compute: { kind: "bloblang" },
      output: { type: "AirQualityObservedAggregate", mode: "upsert" },
      targetEndpoint: "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air",
    });
    expect((firstShape(body.spec).compute as { bloblang: string }).bloblang).toContain("pm10Sum");
  });

  it("shows only the fields of the chosen source, compute and execution (T-2754)", async () => {
    renderPipelines();
    const dialog = await openNew();
    const studio = within(dialog).getByTestId("pipeline-studio");
    // The form's own copies: the studio picks the source, so these are the form's leftovers.
    const formField = (name: string) => dialog.querySelector(`#root_${name}`) as HTMLElement;
    const geo = () => formField("source_query_geoQ");
    const trigger = () => formField("source_trigger_subscription_type");
    // The form's own fields sit folded under More options, shut when the dialog opens (PL-58).
    const summary = [...dialog.querySelectorAll("summary")].find((one) => one.textContent?.startsWith("More options"));
    const more = summary?.closest("details");
    expect(more).not.toHaveAttribute("open");
    await user.click(summary as HTMLElement);
    expect(more).toHaveAttribute("open");

    // Nothing chosen: no source fields, no compute fields, no schedule.
    expect(geo()).not.toBeVisible();
    expect(trigger()).not.toBeVisible();
    expect(formField("source_dataSourceRef")).not.toBeVisible();
    expect(formField("compute_bloblang")).not.toBeVisible();
    expect(formField("compute_module")).not.toBeVisible();
    expect(formField("schedule")).not.toBeVisible();

    // A feed: nothing of an endpoint's query or trigger.
    await pickDataSource(dialog, "mqtt-mesto");
    expect(geo()).not.toBeVisible();
    expect(trigger()).not.toBeVisible();
    expect(formField("source_endpointRef")).not.toBeVisible();

    // A space: the ways of reading the studio does not offer, and not the feed's picker.
    await user.selectOptions(within(section(studio, "studio-source")).getByLabelText(en.pipelines.studio.sourceKind), "space");
    await user.selectOptions(within(section(studio, "studio-source")).getByLabelText(en.pipelines.studio.space), "ovzdusie");
    expect(geo()).toBeVisible();
    expect(trigger()).toBeVisible();
    expect(formField("source_dataSourceRef")).not.toBeVisible();
    // The entity query is the studio's: the form does not ask it twice.
    expect(formField("source_query_type")).not.toBeVisible();

    // Back to a feed: the endpoint's fields go again.
    await pickDataSource(dialog, "mqtt-mesto");
    expect(geo()).not.toBeVisible();

    // The compute kind shows its own fields only.
    const kind = labelled(dialog, /^Kind/);
    await user.selectOptions(kind, en.pipelines.field.computeChoice.wasm);
    expect(formField("compute_module")).toBeVisible();
    expect(formField("compute_function")).toBeVisible();
    expect(formField("compute_bloblang")).not.toBeVisible();
    expect(formField("compute_mappingRef")).not.toBeVisible();
    await user.selectOptions(kind, en.pipelines.field.computeChoice.mapping);
    expect(formField("compute_mappingRef")).toBeVisible();
    expect(formField("compute_module")).not.toBeVisible();

    // The schedule belongs to a scheduled pipeline.
    await user.selectOptions(
      labelled(dialog, /^Execution/),
      en.pipelines.field.classChoice.scheduled,
    );
    expect(formField("schedule")).toBeVisible();
  });

  it("reads its choices as words and chooses no target for the author (T-2754)", async () => {
    renderPipelines();
    const dialog = await openNew();
    const execution = labelled(dialog, /^Execution/);
    expect(within(execution).getByRole("option", { selected: true })).toHaveTextContent(
      en.pipelines.field.classChoice.auto,
    );
    const words = (select: HTMLElement) =>
      within(select).getAllByRole("option").map((option) => option.textContent);
    for (const raw of ["auto", "resident", "scheduled"]) {
      expect(words(execution)).not.toContain(raw);
    }
    expect(words(labelled(dialog, /^Kind/))).not.toContain("wasm");
    // A write target is the author's choice, never the first endpoint of the list.
    const target = targetPick(dialog);
    expect(within(target).getByRole("option", { selected: true })).toHaveTextContent(en.form.choose);
    // One sentence per checkbox, not one above and one under it.
    expect(within(dialog).queryAllByText(en.pipelines.field.allowFeedbackHint).length).toBeLessThanOrEqual(1);
  });

  it("keeps running or paused in the open and the rest in the row's menu (T-2287)", async () => {
    renderPipelines();
    const row = (await screen.findByText("aq-mqtt-ingest")).closest("tr") as HTMLElement;
    const cell = row.lastElementChild as HTMLElement;

    // The live control, then the one menu with Details first (owner 2026-10-31).
    const open = within(cell).getAllByRole("button");
    expect(open).toHaveLength(2);
    expect(open[1].textContent).toBe("\u22ef");

    await user.click(open[1]);
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      en.grid.details,
      en.pipelines.runs.open,
      en.pipelines.rejected.open,
      // The person's own alerts on this pipeline (T-3261).
      en.alerts.open,
      en.resourceEdit.button,
      // The manifest in the forge, an item instead of an icon column (T-6095).
      en.resourceList.viewSource,
      en.saveAs.button,
      en.workspaces.open.action,
      en.resourceDelete.button,
    ]);
  });

  it("edits an existing pipeline at its own path, keeping what the form does not show", async () => {
    const fetchMock = renderPipelines();

    const row = (await screen.findByText("aq-mqtt-ingest")).closest("tr") as HTMLElement;
    // Edit lives in the row's menu now (T-2287).
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await findFormPage();
    expect(labelled(dialog, /^Name/)).toHaveValue("aq-mqtt-ingest");
    expect(labelled(dialog, /^Name/)).toHaveAttribute("readonly");
    expect(within(dialog).getByText(en.pipelines.bloblangHint)).toBeInTheDocument();
    expect(within(dialog).getByRole("link", { name: /bento\.yaml/ })).toHaveAttribute(
      "href",
      expect.stringMatching(/pipelines\/aq-mqtt-ingest\/bento\.yaml$/),
    );

    await user.type(labelled(dialog, /^Period/), "10s");
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await user.click(within(footer(dialog)).getByRole("button", { name: en.form.check }));
    await waitFor(() =>
      expectOpen(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose })),
    );
    await user.click(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/projects/banskabystrica/pipelines/aq-mqtt-ingest",
    );
    const body = (await request.clone().json()) as { spec: Record<string, unknown>; status?: unknown };
    expect(firstShape(body.spec)).toEqual({ ...EXISTING.spec, period: "10s" });
    expect(body.status).toBeUndefined();
  });

  it("proposes `enabled: false` pasted in the YAML view, as the REST route would (T-0885)", async () => {
    // Stored running: the pause comes from the YAML alone, on the first click, with no Check.
    const running = { ...EXISTING, spec: { ...EXISTING.spec, enabled: true } };
    const fetchMock = renderPipelines([running]);
    const row = (await screen.findByText("aq-mqtt-ingest")).closest("tr") as HTMLElement;
    // Edit lives in the row's menu now (T-2287).
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await findFormPage();
    await user.click(within(dialog).getByRole("tab", { name: "YAML" }));
    const editor = await within(dialog).findByRole("textbox", { name: "YAML" });
    const pasted = { ...running, status: undefined, spec: { ...running.spec, enabled: false } };
    fireEvent.change(editor, { target: { value: stringifyYaml(pasted) } });
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await user.click(within(footer(dialog)).getByRole("button", { name: en.form.check }));
    await waitFor(() =>
      expectOpen(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose })),
    );
    await user.click(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const body = (await writes(fetchMock)[0].clone().json()) as { spec: Record<string, unknown> };
    expect(body.spec.enabled).toBe(false);
    expect(body.spec.period).toBe(EXISTING.spec.period);
  });

  it("opens a pipeline's editor on the change the assistant made, and sends nothing until proposed (AG-77)", async () => {
    const changed = { ...EXISTING, status: undefined, spec: { ...EXISTING.spec, period: "5m" } };
    rememberPrefill("/projects/banskabystrica/pipelines?edit=aq-mqtt-ingest", changed);
    window.history.pushState({}, "", "/projects/banskabystrica/pipelines?edit=aq-mqtt-ingest&draft=aq-mqtt-ingest");
    const fetchMock = renderPipelines();

    const dialog = await findFormPage();
    expect(labelled(dialog, /^Name/)).toHaveValue("aq-mqtt-ingest");
    expect(labelled(dialog, /^Period/)).toHaveValue("5m");
    expect(writes(fetchMock)).toHaveLength(0);

    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await user.click(within(footer(dialog)).getByRole("button", { name: en.form.check }));
    await waitFor(() =>
      expectOpen(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose })),
    );
    await user.click(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose }));
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("PUT");
    expect(new URL(request.url).pathname).toBe("/api/v1/projects/banskabystrica/pipelines/aq-mqtt-ingest");
    expect(firstShape(((await request.clone().json()) as { spec: Record<string, unknown> }).spec)).toEqual(
      firstShape(changed.spec as Record<string, unknown>),
    );
  });

  it("writes what is typed to the pipeline's own draft, and opens the draft the address names (T-0791, AG-61, UI-47)", async () => {
    window.history.pushState({}, "", "/projects/banskabystrica/pipelines?draft=aq-derived");
    const fetchMock = renderPipelines();

    // The address named a draft, so the editor is open on it without a click.
    const dialog = await findFormPage();
    await user.type(labelled(dialog, /^Name/), "aq-derived");

    await waitFor(() => {
      const drafted = fetchMock.mock.calls
        .map((call) => call[0] as Request)
        .find((request) => request.method === "PUT" && request.url.includes("/drafts/Pipeline/aq-derived"));
      expect(drafted).toBeDefined();
    });
  });
});

/**
 * The UI contract of the pipeline dialog (T-1774, UI-04, UI-15, UI-16, UI-44, UI-48): axe over
 * the dialog as it opens and over an existing pipeline, every control reachable in the order it
 * is read, an address the API wrote checked before it is a link, and the four locales on the
 * dialog's own words.
 */
describe("the pipeline editor against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/pipelines");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("has no axe violation with the dialog open on a new pipeline", async () => {
    renderPipelines();
    const dialog = await openNew();
    await expectNoViolations(dialog);
  });

  it("has no axe violation with the dialog open on an existing pipeline", async () => {
    renderPipelines();

    const row = (await screen.findByText("aq-mqtt-ingest")).closest("tr") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await findFormPage();
    await within(dialog).findByRole("link", { name: /bento\.yaml/ });

    await expectNoViolations(dialog);
  });

  it("reaches the controls of the dialog's first step by keyboard in the order they are read", async () => {
    renderPipelines();
    const dialog = await openNew();

    const name = labelled(dialog, /^Name/);
    const step = name.closest("section, form, div[class*=grid]") as HTMLElement;
    await expectTabOrder(userEvent, step);
  });

  /// UI-48: `status.sourceUrl` is written by the workspace, so it is checked before it is a link.
  it("offers no link when the address the API wrote is not one a browser may follow", async () => {
    renderPipelines([
      {
        ...EXISTING,
        status: { phase: "Live", sourceUrl: "javascript:alert(document.cookie)" },
      } as Manifest,
    ]);

    const row = (await screen.findByText("aq-mqtt-ingest")).closest("tr") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await findFormPage();

    // The hint about the Bloblang file stays; only the address is refused.
    expect(within(dialog).getByText(en.pipelines.bloblangHint)).toBeInTheDocument();
    expect(within(dialog).queryByRole("link", { name: /bento\.yaml/ })).toBeNull();
    expect(dialog.querySelector('a[href^="javascript:"]')).toBeNull();
  });

  /// T-4818 (after T-3088): a pipeline with no compute step runs the bento.yaml beside its
  /// manifest. The editor shows that file read-only, an Edit switch opens it, and the proposal
  /// carries the edited file beside the manifest.
  it("shows the bento.yaml a pipeline runs, read-only until Edit, and proposes the edited file", async () => {
    const { compute: _dropped, ...spec } = EXISTING.spec as Record<string, unknown>;
    const file = 'pipeline:\n  processors:\n    - mapping: |\n        root = { "type": "AirQualityObserved" }\n';
    const fetchMock = renderPipelines([{ ...EXISTING, spec } as Manifest], file);
    const row = (await screen.findByText("aq-mqtt-ingest")).closest("tr") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await findFormPage();
    const box = await within(section(dialog, "workbench-mapping-title")).findByLabelText(en.pipelines.workbench.mapping.file);
    await waitFor(() => expect(box).toHaveValue(file));
    expect(box).toHaveAttribute("readonly");
    expect(within(dialog).queryByLabelText(en.pipelines.field.bloblang)).toBeNull();

    await user.click(within(section(dialog, "workbench-mapping-title")).getByLabelText(en.pipelines.workbench.mapping.fileEdit));
    expect(box).not.toHaveAttribute("readonly");
    fireEvent.change(box, { target: { value: `${file}# checked\n` } });
    // Basics first: the name and execution sit above the six steps (T-4818).
    const name = labelled(dialog, /^Name/);
    expect(name.compareDocumentPosition(within(dialog).getByTestId("pipeline-workbench")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // No Check is pressed: the dialog checks by itself once the form rests, and Propose opens.
    await waitFor(() => expectOpen(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose })));
    await user.click(within(footer(dialog)).getByRole("button", { name: en.pipelines.propose }));
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const sent = (await writes(fetchMock)[0].clone().json()) as Record<string, unknown>;
    expect(sent.files).toEqual({ "bento.yaml": `${file}# checked\n` });
    expect(sent.draft).toBeUndefined();
    expect((sent.metadata as { name: string }).name).toBe("aq-mqtt-ingest");
  });

  it("offers the mapping box when the pipeline has no bento.yaml, and says so when it cannot be read", async () => {
    const { compute: _dropped, ...spec } = EXISTING.spec as Record<string, unknown>;
    renderPipelines([{ ...EXISTING, spec } as Manifest]);
    const row = (await screen.findByText("aq-mqtt-ingest")).closest("tr") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await findFormPage();
    expect(await within(section(dialog, "workbench-mapping-title")).findByLabelText(en.pipelines.field.bloblang)).toHaveValue("");
    expect(within(dialog).queryByTestId("pipeline-mapping-file")).toBeNull();
  });

  it("names a mapping file the forge did not answer as failed, never as an empty mapping", async () => {
    const { compute: _dropped, ...spec } = EXISTING.spec as Record<string, unknown>;
    renderPipelines([{ ...EXISTING, spec } as Manifest], 503);
    const row = (await screen.findByText("aq-mqtt-ingest")).closest("tr") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await findFormPage();
    const file = await within(dialog).findByTestId("pipeline-mapping-file");
    expect(await within(file).findByRole("button", { name: /retry|try again/i })).toBeInTheDocument();
    const edit = within(section(dialog, "workbench-mapping-title")).getByLabelText(en.pipelines.workbench.mapping.fileEdit);
    expect(edit).toHaveAttribute("aria-disabled", "true");
    expect(edit).toHaveAccessibleDescription(en.pipelines.workbench.mapping.fileEditDisabled);
    await user.click(edit);
    expect(edit).not.toBeChecked();
  });

  it("asks for no bento.yaml for a pipeline that maps in its own compute step", async () => {
    const fetchMock = renderPipelines();
    const row = (await screen.findByText("aq-mqtt-ingest")).closest("tr") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    const dialog = await findFormPage();
    expect(within(dialog).queryByTestId("pipeline-mapping-file")).toBeNull();
    const asked = fetchMock.mock.calls.map((call) => String((call[0] as Request).url ?? call[0]));
    expect(asked.some((url) => url.endsWith("/bento"))).toBe(false);
  });

  /// T-1775: the sampled entities are the shared `Table`, so the caption names it, the headers
  /// carry their scope, and each tick is named by the entity it ticks.
  it("reads the sampled entities as a table with a caption and a named tick per row", async () => {
    renderPipelines();
    const dialog = await openNew();
    const studio = within(dialog).getByTestId("pipeline-studio");

    await user.selectOptions(within(section(studio, "studio-source")).getByLabelText(en.pipelines.studio.sourceKind), "space");
    await user.selectOptions(within(section(studio, "studio-source")).getByLabelText(en.pipelines.studio.space), "ovzdusie");
    await user.selectOptions(within(section(studio, "studio-entities")).getByLabelText(en.entities.type), "AirQualityObserved");
    await user.click(within(section(studio, "studio-entities")).getByRole("button", { name: en.pipelines.studio.loadSample }));

    const table = await within(section(studio, "studio-entities")).findByRole("table", { name: en.pipelines.studio.sample });
    // A long sample scrolls in its own frame instead of growing the studio (T-1775).
    const frame = within(section(studio, "studio-entities")).getByRole("group", { name: en.pipelines.studio.sample });
    expect(frame.className).toContain("max-h-64");
    const headers = within(table).getAllByRole("columnheader");
    expect(headers.every((header) => header.getAttribute("scope") === "col")).toBe(true);
    expect(headers[0]).toHaveTextContent(en.pipelines.studio.tick);

    const tick = within(table).getByRole("checkbox", { name: SAMPLE[0].id });
    await user.click(tick);
    expect(tick).toBeChecked();
    expect(within(studio).getByText(/1 entity ticked|1 entities ticked/)).toBeInTheDocument();
  });

  it.each(SUPPORTED_LOCALES)("writes the dialog in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    renderPipelines();

    const add = await screen.findByRole("button", { name: i18n.t("pipelines.add") });
    await waitFor(() => expect(add).not.toHaveAttribute("aria-disabled"));
    await user.click(add);
    const dialog = await findFormPage();
    await within(within(section(dialog, "workbench-target-title")).getByLabelText(i18n.t("pipelines.field.targetEndpoint"))).findByRole("option", {
      name: "public-air",
    });

    // The dialog carries the form/YAML tabs; the studio inside it has a set of its own, so both
    // answer to this name and what matters is that the name is the locale's, not English.
    expect(within(dialog).getAllByRole("tab", { name: i18n.t("form.view.form") }).length)
      .toBeGreaterThan(0);
    expectNoRawKeys(dialog);
  });
});
