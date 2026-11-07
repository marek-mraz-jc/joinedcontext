/**
 * T-2709: the pipeline workbench (ADR-N-034, PL-58). Six steps in order, each with its one-line
 * hint and its live output from the operation an agent runs for the same step (PL-63): the sample
 * of the picked source, the mapping's records with each problem at its record and field, the
 * verdict against the target space's model, and the target named with where it lands. The mapping
 * step hints each slot of the model with its type, whether it is required, its enum values and an
 * example; a refused sample says why; the verdict reaches the dialog only for the mapping it ran.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useState } from "react";
import type { JSX } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import type { ReactNode } from "react";
import { PipelineWorkbench, answeredHtml, classOfMapping, exampleOf, flatten, mapperType, readSample, readValidation } from "../src/pages/pipelines/PipelineWorkbench";
import { expectAxeClean, jsonResponse, problem } from "./pageHarness";
import { pageFindings } from "./pageChecks";
import { plainValue } from "../src/pages/pipelines/PipelineStudio";

// The Lifetime step links the space's retention rules; this harness renders the steps without a
// router, so the link is a plain anchor here (T-4842).
vi.mock("../src/components/RecordLink", () => ({
  RecordLink: ({ children }: { children: ReactNode }) => <a href="#space">{children}</a>,
}));

const URN = "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:air-write";

const manifest = (kind: string, name: string, spec: Record<string, unknown>): Manifest => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "ovzdusie" },
  spec,
});

const LINKML = [
  "id: https://banskabystrica.sk/models/air",
  "name: air",
  "classes:",
  "  AirQualityObserved:",
  "    slots: [pm10, quality]",
  "slots:",
  "  pm10:",
  "    range: float",
  "    required: true",
  "    description: Particles under 10 µm.",
  "  quality:",
  "    range: QualityLevel",
  "enums:",
  "  QualityLevel:",
  "    permissible_values:",
  "      good: {}",
  "      poor: {}",
  "",
].join("\n");

const DATASOURCES = [
  manifest("DataSource", "shmu-csv", { type: "http", http: { url: "https://feeds.example/air.csv", format: "csv" } }),
  manifest("DataSource", "hsl-hfp", { type: "mqtt", mqtt: { urls: ["tcp://mqtt.hsl.fi:1883"], topics: ["/hfp/v2/journey/ongoing/vp/#"] } }),
];
const ENDPOINTS = [manifest("Endpoint", "air-write", { contextSpaceRef: "ovzdusie", slug: "ep-air" })];
/** A page of the endpoint, as the gateway answers the person's session. */
const ENDPOINT_PAGE = [{ id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:01", type: "AirQualityObserved" }];
const TARGETS = [{ name: "air-write", urn: URN }];

const MAPPING = [
  'root.id = "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:" + this.station',
  'root.type = "AirQualityObserved"',
  'root.pm10 = { "type": "Property", "value": this.pm10.number() }',
].join("\n");

interface Answers {
  /** Held until it resolves: the sample step still running. */
  sampleGate?: Promise<void>;
  sample?: Response | unknown;
  mapping?: unknown;
  validate?: unknown;
  /** The target model, when a test needs another than LINKML. */
  linkml?: string;
}

interface Seen {
  ops: { name: string; body: Record<string, unknown> }[];
}

function stub(answers: Answers, seen: Seen) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input instanceof Request ? input : new Request(String(input));
      const path = new URL(request.url, window.location.origin).pathname;
      const op = /\/ops\/([a-z_]+)$/.exec(path)?.[1];
      if (op) {
        const body = (await request.clone().json()) as Record<string, unknown>;
        seen.ops.push({ name: op, body });
        if (op === "jc_pipeline_sample_source" && answers.sampleGate) await answers.sampleGate;
        const answer =
          op === "jc_pipeline_sample_source" ? answers.sample : op === "jc_pipeline_try_mapping" ? answers.mapping : answers.validate;
        return answer instanceof Response ? answer : jsonResponse(answer ?? {});
      }
      if (path.startsWith("/api/endpoint/")) {
        seen.ops.push({ name: `read ${path}`, body: {} });
        return jsonResponse(ENDPOINT_PAGE);
      }
      if (path.endsWith("/spaces")) {
        return jsonResponse({
          items: [manifest("ContextSpace", "ovzdusie", { dataModelRef: { kind: "DataModel", name: "air" } })],
        });
      }
      if (path.endsWith("/datamodels")) {
        return jsonResponse({ items: [manifest("DataModel", "air", { linkml: answers.linkml ?? LINKML, classes: ["AirQualityObserved"] })] });
      }
      return jsonResponse({ items: [] });
    }),
  );
}

function Harness({
  start,
  verdicts,
  draftName,
}: {
  start: PipelineForm;
  verdicts: { ok: boolean; bloblang: string }[];
  draftName?: string;
}): JSX.Element {
  const [draft, setDraft] = useState<PipelineForm>(start);
  return (
    <PipelineWorkbench
      project="ovzdusie"
      draft={draft}
      onChange={setDraft}
      dataSources={DATASOURCES}
      endpoints={ENDPOINTS}
      targets={TARGETS}
      toManifest={(form) => ({ kind: "Pipeline", metadata: { name: form.name ?? "air" }, spec: form })}
      onVerdict={(ok, bloblang) => verdicts.push({ ok, bloblang })}
      draftName={draftName}
    >
      <details>
        <summary>{en.pipelines.workbench.advanced}</summary>
        <p>advanced editor</p>
      </details>
    </PipelineWorkbench>
  );
}

function show(start: PipelineForm, verdicts: { ok: boolean; bloblang: string }[] = [], draftName?: string) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <Harness start={start} verdicts={verdicts} draftName={draftName} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const step = (title: string) => screen.getByRole("region", { name: new RegExp(`${title}$`) });

const SAMPLE = {
  records: [
    { station: "01", pm10: "18.2" },
    { station: "02", pm10: "x" },
  ],
  fields: ["pm10", "station"],
  count: 2,
  truncated: false,
  errors: [],
};
const MAPPED = {
  records: [
    { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:01", type: "AirQualityObserved", pm10: { type: "Property", value: 18.2 } },
    { id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:02", type: "AirQualityObserved", pm10: { type: "Property", value: "x" } },
  ],
  errors: [],
};
const ONE_BAD = {
  space: "ovzdusie",
  model: "air",
  version: "1.0.0",
  valid: 1,
  rejected: 1,
  verdicts: [
    { index: 0, ok: true, problems: [] },
    { index: 1, ok: false, problems: [{ rule: "sh:datatype", path: "pm10", message: "pm10 is not of the slot's datatype xsd:float" }] },
  ],
};

const STREAMED = {
  streamed: true,
  seconds: 5,
  records: [{ VP: { veh: 1, lat: 60.17 } }, { VP: { veh: 2, lat: 60.18 } }],
  fields: ["VP"],
  count: 2,
  truncated: false,
  errors: [],
};

describe("the pipeline workbench", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  // T-4841: an MQTT source has no request to answer; the workbench listens to it for a few seconds,
  // says so while it waits, and tries the mapping on the messages one each.
  it("listens to a streaming source, says it is waiting, then tries the mapping on its messages", async () => {
    const seen: Seen = { ops: [] };
    let release = () => undefined as void;
    stub({ sample: STREAMED, mapping: MAPPED, validate: ONE_BAD, sampleGate: new Promise<void>((done) => (release = done)) }, seen);
    show({ name: "hfp", source: { dataSourceRef: "hsl-hfp" }, compute: { kind: "bloblang", bloblang: MAPPING }, targetEndpoint: URN } as PipelineForm);

    const sample = step(en.pipelines.workbench.sample.title);
    expect(await within(sample).findByRole("status")).toHaveTextContent(
      "Listening to hsl-hfp for messages: 0 of 5 s, up to 20 messages",
    );
    expect(seen.ops.find((op) => op.name === "jc_pipeline_sample_source")?.body).toEqual({ dataSource: "hsl-hfp" });
    release();
    await within(sample).findByRole("table", { name: en.pipelines.workbench.sample.caption });
    expect(within(sample).getByText("2 of 20 messages arrived within 5 s, with 2 fields")).toBeInTheDocument();
    // The message's members are the columns, by path: values, not the message's JSON.
    expect(within(sample).getByRole("columnheader", { name: "VP.veh" })).toBeInTheDocument();
    expect(within(sample).getByRole("cell", { name: "60.18" })).toBeInTheDocument();
    await waitFor(() => expect(seen.ops.map((op) => op.name)).toContain("jc_pipeline_try_mapping"));
    const tried = seen.ops.find((op) => op.name === "jc_pipeline_try_mapping")?.body;
    expect(tried?.messages).toEqual(STREAMED.records);
    expect(tried).not.toHaveProperty("sample");

    await userEvent.click(within(sample).getByRole("button", { name: en.pipelines.workbench.sample.listenAgain }));
    await waitFor(() => expect(seen.ops.filter((op) => op.name === "jc_pipeline_sample_source")).toHaveLength(2));
    await expectAxeClean(sample);
  });

  it("says a quiet topic sent nothing, and why that may be, with a way to listen again", async () => {
    const seen: Seen = { ops: [] };
    stub({ sample: { ...STREAMED, records: [], fields: [], count: 0 } }, seen);
    show({ name: "hfp", source: { dataSourceRef: "hsl-hfp" }, targetEndpoint: URN } as PipelineForm);

    const sample = step(en.pipelines.workbench.sample.title);
    expect(await within(sample).findByText("No message arrived within 5 s")).toBeInTheDocument();
    expect(within(sample).getByText(en.pipelines.workbench.sample.streamEmpty)).toBeInTheDocument();
    await userEvent.click(within(sample).getByRole("button", { name: en.pipelines.workbench.sample.listenAgain }));
    await waitFor(() => expect(seen.ops.filter((op) => op.name === "jc_pipeline_sample_source")).toHaveLength(2));
    expect(seen.ops.map((op) => op.name)).not.toContain("jc_pipeline_try_mapping");
  });

  it("reads an endpoint source's page with the session and tries the mapping only once the sample is in", async () => {
    // The runner fetched the endpoint's URL with no credential (401), and the mapping's run
    // started beside the sample's and met "a pipeline test is already running" (T-3088).
    const seen: Seen = { ops: [] };
    let release = () => undefined as void;
    stub({ sample: SAMPLE, mapping: MAPPED, validate: ONE_BAD, sampleGate: new Promise<void>((done) => (release = done)) }, seen);
    show({
      name: "air",
      source: { endpointRef: "air-write", query: { type: "AirQualityObserved" } },
      compute: { kind: "bloblang", bloblang: MAPPING },
      targetEndpoint: URN,
    } as PipelineForm);
    await waitFor(() => expect(seen.ops.map((op) => op.name)).toContain("jc_pipeline_sample_source"));
    const sampled = seen.ops.find((op) => op.name === "jc_pipeline_sample_source");
    expect(sampled?.body).toEqual({ sample: { text: JSON.stringify(ENDPOINT_PAGE), format: "json" } });
    expect(seen.ops.map((op) => op.name)).toContain("read /api/endpoint/ep-air/ngsi-ld/v1/entities");
    await new Promise((resolve) => setTimeout(resolve, 800));
    expect(seen.ops.map((op) => op.name)).not.toContain("jc_pipeline_try_mapping");
    release();
    await waitFor(() => expect(seen.ops.map((op) => op.name)).toContain("jc_pipeline_try_mapping"));
    const tried = seen.ops.find((op) => op.name === "jc_pipeline_try_mapping");
    expect((tried?.body as { sample?: unknown }).sample).toEqual({ text: JSON.stringify(ENDPOINT_PAGE), format: "json" });
  });

  // T-3211: a person new to Bloblang who picks an existing data source starts from a mapping that
  // runs, drafted from the source's first record, as a dropped file already drafted one.
  it("drafts the mapping from a picked source's first record, typed as the target model's one class", async () => {
    const seen: Seen = { ops: [] };
    stub({ sample: SAMPLE, mapping: MAPPED, validate: ONE_BAD }, seen);
    show({ targetEndpoint: URN });

    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.workbench.source.pick), "datasource:shmu-csv");
    const mapping = screen.getByLabelText(en.pipelines.field.bloblang) as HTMLTextAreaElement;
    await waitFor(() => expect(mapping.value).toContain('root.type = "AirQualityObserved"'));
    expect(mapping.value).toContain('"urn:ngsi-ld:%v:%v:%v:%v".format("AirQualityObserved", $domain, "ovzdusie", this.station.string())');
    expect(mapping.value).toContain('root.pm10 = { "type": "Property", "value": this.pm10.number().catch(this.pm10) }');
    // The drafted mapping is tried like a typed one.
    await waitFor(() => expect(seen.ops.map((op) => op.name)).toContain("jc_pipeline_try_mapping"));
  });

  it("never replaces a mapping the person has written", async () => {
    stub({ sample: SAMPLE, mapping: MAPPED, validate: ONE_BAD }, { ops: [] });
    show({ targetEndpoint: URN, compute: { kind: "bloblang", bloblang: MAPPING } });

    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.workbench.source.pick), "datasource:shmu-csv");
    await within(step(en.pipelines.workbench.sample.title)).findByRole("table", { name: en.pipelines.workbench.sample.caption });
    expect(screen.getByLabelText(en.pipelines.field.bloblang)).toHaveValue(MAPPING);
  });

  it("adds a slot's example line to the mapping with one click", async () => {
    stub({ sample: SAMPLE, mapping: MAPPED, validate: ONE_BAD }, { ops: [] });
    show({ targetEndpoint: URN, source: { dataSourceRef: "shmu-csv" }, compute: { kind: "bloblang", bloblang: MAPPING } });

    const add = await screen.findByRole("button", { name: "Add the line for quality to the mapping" });
    await userEvent.click(add);
    const value = (screen.getByLabelText(en.pipelines.field.bloblang) as HTMLTextAreaElement).value;
    expect(value.startsWith(MAPPING)).toBe(true);
    expect(value.split("\n").at(-1)).toBe('root.quality = { "type": "Property", "value": "good" }');
  });

  it("shows_seven_steps_in_order_each_with_its_hint", () => {
    stub({}, { ops: [] });
    show({});
    const titles = screen
      .getAllByRole("region")
      .filter((region) => region.tagName === "SECTION")
      .map((region) => region.getAttribute("aria-labelledby"));
    expect(titles).toEqual([
      "workbench-source-title",
      "workbench-sample-title",
      // The target before the mapping, so the mapping opens knowing the model (T-3296).
      "workbench-target-title",
      "workbench-mapping-title",
      "workbench-output-title",
      "workbench-lifetime-title",
      "workbench-validation-title",
    ]);
    for (const hint of [
      en.pipelines.workbench.source.hint,
      en.pipelines.workbench.sample.hint,
      en.pipelines.workbench.mapping.hint,
      en.pipelines.workbench.output.hint,
      en.pipelines.workbench.validation.hint,
      en.pipelines.workbench.target.hint,
    ]) {
      expect(screen.getByText(hint)).toBeInTheDocument();
    }
    // Nothing runs before a source is picked, and each step says what it waits for.
    expect(screen.getByText(en.pipelines.workbench.sample.waiting)).toBeInTheDocument();
    // The advanced editor is folded until opened.
    expect(screen.getByText("advanced editor")).not.toBeVisible();
  });

  it("samples_the_picked_source_then_tries_the_mapping_and_points_each_problem_at_its_record_and_field", async () => {
    const seen: Seen = { ops: [] };
    const verdicts: { ok: boolean; bloblang: string }[] = [];
    stub({ sample: SAMPLE, mapping: MAPPED, validate: ONE_BAD }, seen);
    const { container } = show({ targetEndpoint: URN, compute: { kind: "bloblang", bloblang: MAPPING } }, verdicts);

    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.workbench.source.pick), "datasource:shmu-csv");
    const sample = await within(step(en.pipelines.workbench.sample.title)).findByRole("table", {
      name: en.pipelines.workbench.sample.caption,
    });
    expect(within(sample).getAllByRole("columnheader").map((cell) => cell.textContent)).toEqual(["pm10", "station"]);
    expect(within(sample).getAllByRole("row")).toHaveLength(3);
    expect(screen.getByText("2 records with 2 fields")).toBeInTheDocument();

    const output = step(en.pipelines.workbench.output.title);
    // Each record is a table of its attributes (T-4842): the value, the verdict at its field.
    const first = await within(output).findByTestId("workbench-record-0");
    const second = within(output).getByTestId("workbench-record-1");
    expect(within(second).getByRole("table", { name: /urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:02/ })).toBeInTheDocument();
    // The second record breaks the model: its pm10 row names the rule.
    expect(await within(second).findByText("sh:datatype")).toBeInTheDocument();
    const pm10 = within(second).getAllByRole("row").find((row) => within(row).queryByText('"x"'));
    expect(pm10 && within(pm10).getByText("sh:datatype")).toBeTruthy();
    expect(within(first).getByText(en.pipelines.workbench.validation.valid)).toBeInTheDocument();
    expect(within(first).getByText("18.2")).toBeInTheDocument();

    const verdict = within(step(en.pipelines.workbench.validation.title)).getByRole("status");
    expect(verdict).toHaveTextContent("1 record breaks air 1.0.0 and would not be written; 1 valid.");
    await waitFor(() => expect(verdicts.at(-1)).toEqual({ ok: false, bloblang: MAPPING }));

    // Every step ran the operation an agent runs, with the source and the sample it names.
    // The Target step asks the same operation which attributes the Endpoint grants (T-4842).
    const judged = seen.ops.filter((op) => op.body.type !== undefined);
    expect(judged.map((op) => [op.name, op.body.type, op.body.records])).toEqual([["jc_pipeline_validate", "AirQualityObserved", []]]);
    const ops = seen.ops.filter((op) => op.body.type === undefined);
    expect(ops.map((op) => op.name)).toEqual(["jc_pipeline_sample_source", "jc_pipeline_try_mapping", "jc_pipeline_validate"]);
    expect(ops[0].body).toEqual({ dataSource: "shmu-csv" });
    expect(ops[1].body.sample).toEqual({ url: "https://feeds.example/air.csv", format: "csv" });
    expect((ops[2].body.records as unknown[]).length).toBe(2);
    await expectAxeClean(container);
  });

  it("shows a nested sample value in words, never as JSON (T-4972)", async () => {
    // An XML source's root element holds its namespaces: Bento reads `xmlns:gml` as `-gml`.
    const XML = {
      records: [{ FeatureCollection: { "-BsWfs": "http://xml.fmi.fi/schema/wfs/2.0", "-gml": "http://www.opengis.net/gml/3.2" }, members: [1, 2] }],
      fields: ["FeatureCollection", "members"],
      count: 1,
      truncated: false,
      errors: [],
    };
    stub({ sample: XML, mapping: MAPPED, validate: ONE_BAD }, { ops: [] });
    show({ targetEndpoint: URN, compute: { kind: "bloblang", bloblang: MAPPING } });

    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.workbench.source.pick), "datasource:shmu-csv");
    const sample = await within(step(en.pipelines.workbench.sample.title)).findByRole("table", {
      name: en.pipelines.workbench.sample.caption,
    });
    expect(within(sample).getByRole("cell", { name: "-BsWfs: http://xml.fmi.fi/schema/wfs/2.0; -gml: http://www.…" })).toBeInTheDocument();
    expect(within(sample).getByRole("cell", { name: "1, 2" })).toBeInTheDocument();
    expect(pageFindings({ namespaces: [], layout: false }).filter((finding) => finding.startsWith("raw-json"))).toEqual([]);
  });

  it("reads a nested value as key and value pairs, a list as its items", () => {
    expect(plainValue({ a: 1, b: { c: [true, null], d: "x" } })).toBe("a: 1; b: (c: true, null; d: x)");
    expect(plainValue(undefined)).toBe("");
    expect(plainValue([[[[1]]]])).toBe("…");
  });

  it("gives_a_green_verdict_only_for_the_mapping_that_ran_and_only_when_every_record_is_valid", async () => {
    const verdicts: { ok: boolean; bloblang: string }[] = [];
    stub(
      {
        sample: SAMPLE,
        mapping: { records: MAPPED.records.slice(0, 1), errors: [] },
        validate: { ...ONE_BAD, valid: 1, rejected: 0, verdicts: [ONE_BAD.verdicts[0]] },
      },
      { ops: [] },
    );
    show({ targetEndpoint: URN, source: { dataSourceRef: "shmu-csv" }, compute: { kind: "bloblang", bloblang: MAPPING } }, verdicts);
    expect(
      await within(step(en.pipelines.workbench.validation.title)).findByText(
        "1 record is valid against air 1.0.0.",
      ),
    ).toBeInTheDocument();
    await waitFor(() => expect(verdicts.at(-1)).toEqual({ ok: true, bloblang: MAPPING }));
  });

  it("names_where_each_record_would_be_refused_and_the_expiry_it_would_get", async () => {
    const seen: Seen = { ops: [] };
    const checked = {
      ...ONE_BAD,
      verdicts: [
        { index: 0, ok: true, problems: [], endpoint: "helsinki-all", findings: [], expires: { entity: "2026-10-20T12:08:00Z", attributes: { pm10: "2026-10-20T12:05:00Z" } } },
        {
          index: 1,
          ok: false,
          problems: [],
          endpoint: "helsinki-all",
          findings: [
            { source: "endpoint", rule: "NOT_WRITABLE", field: "expiresAt", reason: "not writable through helsinki-all" },
            { source: "lifetime", rule: "from", field: "observedAt", reason: "the record gives no date-time in observedAt" },
          ],
        },
      ],
    };
    stub({ sample: SAMPLE, mapping: MAPPED, validate: checked }, seen);
    show({ targetEndpoint: URN, compute: { kind: "bloblang", bloblang: MAPPING } }, [], "air-draft");
    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.workbench.source.pick), "datasource:shmu-csv");
    const output = step(en.pipelines.workbench.output.title);
    const first = await within(output).findByTestId("workbench-record-0");
    const second = within(output).getByTestId("workbench-record-1");
    expect(await within(second).findByText("not writable through helsinki-all")).toBeInTheDocument();
    expect(within(second).getByText(en.pipelines.workbench.validation.source.endpoint)).toBeInTheDocument();
    expect(within(second).getByText(en.pipelines.workbench.validation.source.lifetime)).toBeInTheDocument();
    expect(within(second).getByText("2 problems")).toBeInTheDocument();
    // The ends it would be written with: the entity's and the attribute's, in the Expires column.
    const entity = within(first).getAllByRole("row").find((row) => within(row).queryByText("expiresAt"));
    expect(entity).toHaveTextContent("2026-10-20T12:08:00Z");
    const pm10 = within(first).getAllByRole("row").find((row) => within(row).queryByText("pm10"));
    expect(pm10).toHaveTextContent("2026-10-20T12:05:00Z");
    // The run of the mapping keeps its record check on the dialog's draft, for the proposal;
    // the records shown back to the validation step keep nothing (T-4840).
    const tried = seen.ops.find((op) => op.name === "jc_pipeline_try_mapping");
    expect(tried?.body.draft).toEqual({ kind: "Pipeline", name: "air-draft" });
    await waitFor(() => expect(seen.ops.at(-1)?.name).toBe("jc_pipeline_validate"));
    expect(seen.ops.at(-1)?.body.draft).toBeUndefined();
  });

  it("names_each_slot_of_the_target_model_with_its_type_whether_required_its_values_and_an_example", async () => {
    stub({}, { ops: [] });
    show({ targetEndpoint: URN, compute: { kind: "bloblang", bloblang: MAPPING } });
    const mapping = step(en.pipelines.workbench.mapping.title);
    const slots = await within(mapping).findByRole("region", { name: "What AirQualityObserved takes" });
    // The list scrolls on its own, so it is a named keyboard stop (axe scrollable-region-focusable, T-3144).
    const list = within(slots).getByRole("list", { name: "What AirQualityObserved takes" });
    expect(list).toHaveAttribute("tabindex", "0");
    const [pm10, quality] = within(slots).getAllByRole("listitem");
    expect(within(pm10).getByText("float")).toBeInTheDocument();
    expect(within(pm10).getByText(en.pipelines.workbench.mapping.required)).toBeInTheDocument();
    expect(within(pm10).getByText("Particles under 10 µm.")).toBeInTheDocument();
    expect(within(pm10).getByText(/"value": 3\.5/)).toBeInTheDocument();
    expect(within(quality).getByText("Values: good, poor")).toBeInTheDocument();
    expect(within(quality).queryByText(en.pipelines.workbench.mapping.required)).toBeNull();
    expect(within(quality).getByText(/"value": "good"/)).toBeInTheDocument();
    // The target names the space the records land in and the model that checks them.
    expect(
      within(step(en.pipelines.workbench.target.title)).getByText(
        "The records land in the space ovzdusie, checked against the model air.",
      ),
    ).toBeInTheDocument();
  });

  // T-4971: a model of several classes asks which one the mapping writes; that picker carries its
  // one-line hint like every other field (UI-16, the walker's field-hint check).
  it("the_class_picker_of_a_model_with_several_classes_carries_its_hint", async () => {
    const twoClasses = LINKML.replace("classes:\n", "classes:\n  WeatherObserved:\n    slots: [pm10]\n");
    stub({ linkml: twoClasses }, { ops: [] });
    show({ targetEndpoint: URN, compute: { kind: "bloblang", bloblang: MAPPING } });
    const mapping = step(en.pipelines.workbench.mapping.title);
    const picker = await within(mapping).findByRole("combobox", { name: en.pipelines.workbench.mapping.type });
    expect(picker).toHaveAccessibleDescription(en.pipelines.workbench.mapping.typeHint);
    expect(en.pipelines.workbench.mapping.typeHint).toMatch(/for example/);
  });

  // T-3296: the target is step 3, so a newcomer picks it before writing the mapping, and the
  // mapping step shows what the target's model takes without a jump ahead.
  it("the_mapping_shows_the_target_class_slots_once_the_target_above_it_is_picked", async () => {
    stub({}, { ops: [] });
    show({ compute: { kind: "bloblang", bloblang: MAPPING } });
    const mapping = step(en.pipelines.workbench.mapping.title);
    expect(within(mapping).getByText(en.pipelines.workbench.mapping.pickTarget)).toBeInTheDocument();
    expect(en.pipelines.workbench.mapping.pickTarget).toMatch(/step 3/);
    await userEvent.selectOptions(screen.getByLabelText(en.pipelines.field.targetEndpoint), URN);
    expect(await within(mapping).findByRole("region", { name: "What AirQualityObserved takes" })).toBeInTheDocument();
    expect(within(mapping).queryByText(en.pipelines.workbench.mapping.pickTarget)).toBeNull();
  });

  it("a_space_without_a_model_says_so_and_does_not_hold_the_proposal", async () => {
    const verdicts: { ok: boolean; bloblang: string }[] = [];
    stub(
      {
        sample: SAMPLE,
        mapping: MAPPED,
        validate: new Response(
          JSON.stringify({ error: "no_model", space: "ovzdusie", detail: "space 'ovzdusie' names no data model, so nothing checks what lands in it; name its model in spec.dataModelRef (DM-61)" }),
          { status: 409, headers: { "Content-Type": "application/json" } },
        ),
      },
      { ops: [] },
    );
    show({ targetEndpoint: URN, source: { dataSourceRef: "shmu-csv" }, compute: { kind: "bloblang", bloblang: MAPPING } }, verdicts);
    expect(
      await within(step(en.pipelines.workbench.validation.title)).findByText(/names no data model/),
    ).toBeInTheDocument();
    await waitFor(() => expect(verdicts.at(-1)).toEqual({ ok: true, bloblang: MAPPING }));
  });

  it("a_sample_the_source_refuses_says_why_and_the_mapping_waits", async () => {
    const seen: Seen = { ops: [] };
    stub(
      {
        sample: problem(400, "DataSource 'shmu-csv' cannot be sampled: the source declares a credential; give the sample as text (a file of the feed) or as a URL instead"),
      },
      seen,
    );
    show({ source: { dataSourceRef: "shmu-csv" } });
    const alert = await within(step(en.pipelines.workbench.sample.title)).findByRole("alert");
    expect(alert).toHaveTextContent("give the sample as text");
    expect(screen.getByText(en.pipelines.workbench.output.waiting)).toBeInTheDocument();
    expect(seen.ops.map((op) => op.name)).toEqual(["jc_pipeline_sample_source"]);
  });

  it("a_dropped_file_is_the_sample_and_drafts_an_empty_mapping", async () => {
    const seen: Seen = { ops: [] };
    stub({ sample: SAMPLE, mapping: MAPPED, validate: ONE_BAD }, seen);
    show({ targetEndpoint: URN });
    const file = new File(["station,pm10\n01,18.2\n"], "air-quality.csv", { type: "text/csv" });
    await userEvent.upload(screen.getByLabelText(en.pipelines.workbench.source.file), file);
    expect(await screen.findByText("Sample: air-quality.csv")).toBeInTheDocument();
    const editor = screen.getByLabelText(en.pipelines.field.bloblang);
    await waitFor(() => expect((editor as HTMLTextAreaElement).value).toContain("root.type"));
    await waitFor(() =>
      expect(seen.ops.find((op) => op.name === "jc_pipeline_sample_source")?.body).toEqual({ sample: { text: "station,pm10\n01,18.2\n", format: "csv" } }),
    );
  });

  it("is_worked_from_the_keyboard_in_the_order_of_the_steps", async () => {
    stub({}, { ops: [] });
    show({});
    await userEvent.tab();
    expect(screen.getByLabelText(en.pipelines.workbench.source.pick)).toHaveFocus();
    await userEvent.tab();
    expect(screen.getByLabelText(en.pipelines.workbench.source.file)).toHaveFocus();
    // Step 3 is the target (T-3296); step 4 asks how a record becomes an entity (T-3224).
    await userEvent.tab();
    expect(screen.getByLabelText(en.pipelines.field.targetEndpoint)).toHaveFocus();
    await userEvent.tab();
    expect(screen.getByRole("radio", { name: en.pipelines.mapper.modeCode })).toHaveFocus();
    await userEvent.tab();
    expect(screen.getByLabelText(en.pipelines.field.bloblang)).toHaveFocus();
  });
});

describe("what the workbench reads from an answer", () => {
  it("opens a message's objects into columns by path, two levels deep, lists whole", () => {
    expect(flatten({ VP: { veh: 1, pos: { lat: 60.1, deep: { x: 1 } } }, tags: [1, 2] })).toEqual({
      "VP.veh": 1,
      "VP.pos.lat": 60.1,
      "VP.pos.deep": { x: 1 },
      tags: [1, 2],
    });
  });

  it("reads_an_answer_of_another_shape_as_nothing_rather_than_breaking", () => {
    expect(readSample({ kind: "Change" })).toEqual({ records: [], fields: [], count: 0, truncated: false, errors: [], streamed: false, seconds: 0, messages: [] });
    expect(readSample(null).records).toEqual([]);
    expect(readValidation({ verdicts: [{ index: "x", ok: "yes", problems: [null] }] }).verdicts).toEqual([
      { index: 0, ok: false, problems: [] },
    ]);
  });

  it("names_the_class_a_mapping_writes_and_an_example_per_range", () => {
    expect(classOfMapping('root.type = "Tram"', ["Bus", "Tram"])).toBe("Tram");
    // A type the model lacks is no reason to show another one's slots (T-4842).
    expect(classOfMapping('root.type = "Ferry"', ["Bus", "Tram"])).toBeUndefined();
    expect(classOfMapping('root.type = "Ferry"', ["Bus"])).toBe("Bus");
    // The output's declared type wins over the first class the text names.
    expect(classOfMapping('"type": "Event"', ["Event", "Vehicle"], "Vehicle")).toBe("Vehicle");
    expect(classOfMapping("", ["Event", "Vehicle"], "Ferry")).toBeUndefined();
    expect(classOfMapping("", [])).toBeUndefined();
    // T-4818: a bento.yaml builds whole entities, `"type": "…"`; the first known class wins.
    const bento = 'root = $rows.map_each(r -> { "type": "Property", "id": r.id })\n  "type": "AirQualityObserved",';
    expect(classOfMapping(bento, ["Event", "AirQualityObserved"])).toBe("AirQualityObserved");
    expect(classOfMapping('"type": "Ferry"', ["Event", "Tram"])).toBeUndefined();
    expect(mapperType('# jc-mapper {"type":"Vehicle","attributes":{}}\nroot = {}')).toBe("Vehicle");
    expect(mapperType('# jc-mapper {not json\n')).toBeUndefined();
    expect(mapperType('root.type = "Vehicle"')).toBeUndefined();
    expect(exampleOf({ range: "integer", kind: "Property" })).toBe("12");
    // T-2861: a relationship's example is an id of its target class, a list on a many end.
    expect(exampleOf({ range: "Station", kind: "Relationship" })).toBe('"urn:ngsi-ld:Station:…"');
    expect(exampleOf({ range: "Course", kind: "Relationship", multivalued: true })).toBe('["urn:ngsi-ld:Course:…"]');
    expect(exampleOf({ range: "Level", kind: "Property" }, "good")).toBe('"good"');
  });
});

describe("a sample the source answered as a web page (T-4165, photo audit T-4159)", () => {
  it("is told apart from any other engine failure", () => {
    expect(
      answeredHtml(
        "failed assignment (line 1): function content: failed to parse value as JSON: invalid character '<' looking for beginning of value",
      ),
    ).toBe(true);
    expect(answeredHtml("Unexpected token '<', \"<!doctype \"... is not valid JSON")).toBe(true);
    expect(answeredHtml("failed assignment (line 3): expected number value, got string")).toBe(false);
  });
});
