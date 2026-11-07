/**
 * T-0591: a candidate pipeline tried on a dropped sample (PL-43, PL-44, UI-32): the file drafts
 * the type and the mapping, the test runs through the Portal's route, the trace is drawn as
 * stages with every error at its line, and the proposal is the dialog's own Propose.
 */
import { useState } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { expectDenied, expectNoRawKeys, expectNoViolations, expectTabOrder, expectOpen } from "./checks";
import en from "../src/locales/en.json";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import type { Manifest } from "../src/api/manifest";
import { greenVerdict, isCheck } from "./verdict";
import { sentTo } from "./requests";

function MockEditor({ value, onChange }: { value: string; onChange?: (value: string) => void }) {
  return <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />;
}
vi.mock("../src/pages/models/MonacoSourceView", () => ({ default: MockEditor }));

const { PipelineStudio } = await import("../src/pages/pipelines/PipelineStudio");
const { PipelineEditorDialog } = await import("../src/pages/pipelines/PipelineEditor");
const { draftFromSample, errorLines, idColumnOf, spaceOfTarget, typeOf } = await import(
  "../src/pages/pipelines/PipelineTest"
);

const CSV = "station_id,pm10,timestamp\n01,18.2,2026-09-13T07:00:00Z\n02,x,2026-09-13T07:00:00Z\n";
const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all";

const TRACE = {
  input: { events: 2, bytes: 90, sample: { station_id: "01", pm10: "18.2", timestamp: "2026-09-13T07:00:00Z" } },
  mapping: [{ id: "urn:ngsi-ld:AirQuality:hel.fi:helsinki:01", type: "AirQuality", pm10: { type: "Property", value: 18.2 } }],
  validation: [{ index: 0, ok: true, problems: [] }],
  errors: [{ stage: "mapping", line: 3, message: "failed assignment (line 3): expected number" }],
};

const CLEAN = { ...TRACE, errors: [] };

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

function mockFetch(test: { status: number; body: unknown }) {
  const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const method = init?.method ?? (input instanceof Request ? input.method : "GET");
    const json = (body: unknown, status = 200) =>
      Promise.resolve(new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }));
    if (url.includes("/pipelines/test") && method === "POST") {
      return json(test.body, test.status);
    }
    // The dialog's Check is a dry run of the collection; it answers a verdict fresh for the
    // manifest it judged, without which strict validation proposes nothing (AG-62, T-0779).
    if (input instanceof Request && isCheck(input, new URL(url))) {
      return greenVerdict(input).then((body) => json(body));
    }
    return json(list([]));
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

const HTTP_SOURCE: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataSource",
  metadata: { name: "hsl-citybikes-free", namespace: "helsinki" },
  spec: { type: "http", http: { url: "https://gbfs.example.org/free_bike_status.json", timeout: "15s" } },
};

function Harness({
  initial,
  onForm,
  dataSources = [],
}: {
  initial?: PipelineForm;
  onForm: (form: PipelineForm) => void;
  dataSources?: Manifest[];
}) {
  const [draft, setDraft] = useState<PipelineForm | undefined>(initial);
  return (
    <PipelineStudio
      project="helsinki"
      draft={draft}
      onChange={(form) => {
        setDraft(form);
        onForm(form);
      }}
      dataSources={dataSources}
      endpoints={[]}
      toManifest={(form) => ({ kind: "Pipeline", metadata: { name: form.name }, spec: form })}
    />
  );
}

function renderStudio(initial?: PipelineForm, dataSources?: Manifest[]) {
  const onForm = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <Harness initial={initial} onForm={onForm} dataSources={dataSources} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return onForm;
}

function csvFile(text = CSV, name = "air_quality.csv"): File {
  return new File([text], name, { type: "text/csv" });
}

async function drop(file: File) {
  await userEvent.upload(screen.getByLabelText(en.pipelines.test.chooseFile), file);
}

describe("drafting from a sample (PL-44)", () => {
  it("drafts the type from the file name, the id from the id column and a Property per column", () => {
    const draft = draftFromSample("air_quality.csv", CSV, "helsinki");
    expect(draft?.format).toBe("csv");
    expect(draft?.type).toBe("AirQuality");
    expect(draft?.idColumn).toBe("station_id");
    expect(draft?.bloblang).toContain('root.type = "AirQuality"');
    expect(draft?.bloblang).toContain('"helsinki", this.station_id.string()');
    expect(draft?.bloblang).toContain("root.pm10 = { \"type\": \"Property\", \"value\": this.pm10.number().catch(this.pm10) }");
    expect(draft?.bloblang).toContain("root.timestamp = { \"type\": \"Property\", \"value\": this.timestamp }");
    expect(draft?.bloblang).not.toContain("root.station_id");
  });

  it("takes a JSON array's first element, and nothing from text it cannot read", () => {
    const draft = draftFromSample("bikes.json", '[{"id":"b1","free":3},{"id":"b2","free":0}]', "helsinki");
    expect(draft?.format).toBe("json");
    expect(draft?.idColumn).toBe("id");
    expect(draft?.bloblang).toContain("this.free.number()");
    expect(draftFromSample("notes.txt", "just words", "helsinki")).toBeNull();
    expect(draftFromSample("empty.csv", "", "helsinki")).toBeNull();
  });

  it("names things the way the platform does", () => {
    expect(typeOf("2026-air.csv")).toBe("Sample2026Air");
    expect(typeOf("bike stations.json")).toBe("BikeStations");
    expect(idColumnOf(["name", "sensorId", "value"])).toBe("sensorId");
    expect(idColumnOf(["name", "value"])).toBe("name");
    expect(spaceOfTarget(TARGET, "fallback")).toBe("helsinki");
    expect(spaceOfTarget("not-a-urn", "fallback")).toBe("fallback");
    expect([...errorLines(TRACE)]).toEqual([3]);
  });
});

describe("the sample test in the studio (PL-43)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("drafts the mapping from a dropped CSV, runs the test and draws the stages with the error at its line", async () => {
    const fetchMock = mockFetch({ status: 200, body: TRACE });
    const onForm = renderStudio({ name: "shmu-air", class: "resident", targetEndpoint: TARGET });
    await drop(csvFile());

    await waitFor(() => expect(onForm).toHaveBeenCalled());
    const form = onForm.mock.calls.at(-1)?.[0] as PipelineForm;
    expect(form.compute?.kind).toBe("bloblang");
    expect(form.compute?.bloblang).toContain("this.station_id");
    expect(form.output?.type).toBe("AirQuality");
    expect(screen.getByText(/air_quality\.csv · csv/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: en.pipelines.test.run }));
    const call = await waitFor(() => {
      const [found] = sentTo(fetchMock, "/pipelines/test");
      expect(found).toBeDefined();
      return found;
    });
    expect(call.path).toBe("/api/v1/projects/helsinki/pipelines/test");
    expect(call.method).toBe("POST");
    const body = (await call.json()) as { pipeline: { spec: PipelineForm }; sample: { text: string; format: string } };
    expect(body.sample).toEqual({ text: CSV, format: "csv" });
    expect(body.pipeline.spec.compute?.bloblang).toContain("this.station_id");

    const inspector = await screen.findByTestId("stage-inspector");
    // T-5516: the input sample reads as rows, never as JSON text.
    expect(within(inspector).getAllByText("station_id").length).toBeGreaterThan(0);
    expect(inspector.textContent).not.toContain("{\n");
    expect(within(inspector).getByText(/urn:ngsi-ld:AirQuality:hel.fi:helsinki:01/)).toBeInTheDocument();
    expect(within(inspector).getByText(`#1 ${en.pipelines.test.ok}`)).toBeInTheDocument();
    expect(within(inspector).getByText(/line 3/)).toBeInTheDocument();
    const marked = inspector.querySelector('[data-line="3"]');
    expect(marked?.className).toContain("decoration-danger");
    expect(inspector.querySelector('[data-line="2"]')?.className).not.toContain("decoration-danger");
    expect(screen.queryByText(/Propose the pipeline below/)).toBeNull();
  });

  it("says when every message became an entity, and shows the route's reason when it refuses", async () => {
    mockFetch({ status: 200, body: CLEAN });
    renderStudio({ name: "shmu-air", class: "resident", targetEndpoint: TARGET });
    await drop(csvFile());
    await userEvent.click(await screen.findByRole("button", { name: en.pipelines.test.run }));
    expect(await screen.findByText(/All 1 messages map to entities/)).toBeInTheDocument();

    vi.unstubAllGlobals();
    mockFetch({ status: 503, body: { status: 503, detail: "no pipeline runner is configured" } });
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.test.run }));
    expect(await screen.findByRole("alert")).toHaveTextContent("no pipeline runner is configured");
    expect(screen.queryByTestId("stage-inspector")).toBeNull();
  });

  it("offers the http source's own URL as the sample and lets the runner fetch it (PL-48)", async () => {
    const fetchMock = mockFetch({ status: 200, body: CLEAN });
    renderStudio(
      {
        name: "citybikes-free",
        class: "auto",
        targetEndpoint: TARGET,
        source: { dataSourceRef: "hsl-citybikes-free" },
        compute: { kind: "bloblang", bloblang: "root = this.data.bikes" },
      },
      [HTTP_SOURCE],
    );
    await userEvent.click(await screen.findByRole("button", { name: en.pipelines.test.useUrl }));
    expect(screen.getByText(/free_bike_status\.json · json · fetched by the runner/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.test.run }));
    const call = await waitFor(() => {
      const [found] = sentTo(fetchMock, "/pipelines/test");
      expect(found).toBeDefined();
      return found;
    });
    const body = (await call.json()) as { sample: unknown };
    expect(body.sample).toEqual({ url: "https://gbfs.example.org/free_bike_status.json", format: "json" });
    expect(await screen.findByText(/All 1 messages map to entities/)).toBeInTheDocument();
  });

  // T-5350, PL-114: the test splits a sample the way the live run will. An http source sends its
  // whole body as one message unless its DataSource says `format: csv`; a per-row mapping that
  // passed a row-split test wrote nothing live.
  const CSV_SOURCE: Manifest = {
    ...HTTP_SOURCE,
    metadata: { name: "stations", namespace: "helsinki" },
    spec: { type: "http", http: { url: "https://data.example.org/stations.csv" } },
  };
  const ROWS_SOURCE: Manifest = {
    ...CSV_SOURCE,
    spec: { type: "http", http: { url: "https://data.example.org/stations.csv", format: "csv" } },
  };
  const STATIONS = { name: "stations", class: "auto" as const, targetEndpoint: TARGET, source: { dataSourceRef: "stations" } };

  async function sentSample(fetchMock: ReturnType<typeof mockFetch>): Promise<unknown> {
    await userEvent.click(screen.getByRole("button", { name: en.pipelines.test.run }));
    const call = await waitFor(() => {
      const [found] = sentTo(fetchMock, "/pipelines/test");
      expect(found).toBeDefined();
      return found;
    });
    return ((await call.json()) as { sample: unknown }).sample;
  }

  it("tests a CSV file as one message when the http source sends its whole body, and says why (T-5350)", async () => {
    const fetchMock = mockFetch({ status: 200, body: CLEAN });
    renderStudio(STATIONS, [CSV_SOURCE]);
    await drop(csvFile());
    expect(await screen.findByTestId("test-whole-body")).toHaveTextContent(en.pipelines.test.wholeBody);
    expect(await sentSample(fetchMock)).toEqual({ text: CSV, format: "text" });
  });

  it("tests a CSV row by row when the http source says format: csv (PL-114)", async () => {
    const fetchMock = mockFetch({ status: 200, body: CLEAN });
    renderStudio(STATIONS, [ROWS_SOURCE]);
    await drop(csvFile());
    expect(screen.queryByTestId("test-whole-body")).toBeNull();
    expect(await sentSample(fetchMock)).toEqual({ text: CSV, format: "csv" });
  });

  it("fetches a format: csv source's URL as rows (PL-114)", async () => {
    const fetchMock = mockFetch({ status: 200, body: CLEAN });
    renderStudio({ ...STATIONS, compute: { kind: "bloblang", bloblang: "root = this" } }, [ROWS_SOURCE]);
    await userEvent.click(await screen.findByRole("button", { name: en.pipelines.test.useUrl }));
    expect(screen.getByText(/stations\.csv · csv · fetched by the runner/)).toBeInTheDocument();
    expect(await sentSample(fetchMock)).toEqual({ url: "https://data.example.org/stations.csv", format: "csv" });
  });

  it("offers no URL for a source that is not http", async () => {
    mockFetch({ status: 200, body: CLEAN });
    renderStudio({ name: "x", class: "auto", targetEndpoint: TARGET, source: { dataSourceRef: "mqtt-x" } }, [
      { ...HTTP_SOURCE, metadata: { name: "mqtt-x", namespace: "helsinki" }, spec: { type: "mqtt", http: {} } },
    ]);
    await screen.findByText(en.pipelines.test.title);
    expect(screen.queryByRole("button", { name: en.pipelines.test.useUrl })).toBeNull();
  });

  it("refuses a sample over 5 MiB before anything is sent", async () => {
    const fetchMock = mockFetch({ status: 200, body: TRACE });
    renderStudio({ name: "shmu-air", class: "resident", targetEndpoint: TARGET });
    const big = new File([new Uint8Array(5 * 1024 * 1024 + 1)], "big.csv", { type: "text/csv" });
    await drop(big);
    expect(await screen.findByRole("alert")).toHaveTextContent(en.pipelines.test.tooLarge);
    expect(sentTo(fetchMock, "/pipelines/test")).toEqual([]);
  });
});

describe("from a sample to a proposal", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("proposes the manifest with the drafted mapping through the dialog's own button", async () => {
    mockFetch({ status: 200, body: CLEAN });
    const onSubmit = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <PipelineEditorDialog
            project="helsinki"
            onOpenChange={() => {}}
            editing={null}
            initial={{ name: "shmu-air", class: "resident", targetEndpoint: TARGET }}
            pending={false}
            error={null}
            onSubmit={onSubmit}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const dialog = await screen.findByRole("dialog");
    await userEvent.upload(within(dialog).getByLabelText(en.pipelines.test.chooseFile), csvFile());
    await userEvent.click(await within(dialog).findByRole("button", { name: en.pipelines.test.run }));
    await within(dialog).findByText(/All 1 messages map to entities/);

    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.form.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.pipelines.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.pipelines.propose }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const envelope = onSubmit.mock.calls[0][0] as { kind: string; spec: { compute?: { bloblang?: string }; output?: { type?: string } } };
    expect(envelope.kind).toBe("Pipeline");
    expect(envelope.spec.compute?.bloblang).toContain('root.type = "AirQuality"');
    expect(envelope.spec.output?.type).toBe("AirQuality");
  });

  it("keeps Propose closed, with the reason, until the test is green for the mapping in the editor (PL-49)", async () => {
    mockFetch({ status: 200, body: CLEAN });
    const onSubmit = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <PipelineEditorDialog
            project="helsinki"
            onOpenChange={() => {}}
            editing={null}
            initial={{
              name: "shmu-air",
              class: "resident",
              targetEndpoint: TARGET,
              source: { dataSourceRef: "shmu-csv" },
              compute: { kind: "bloblang", bloblang: "root.id = this.station_id" },
            }}
            pending={false}
            error={null}
            onSubmit={onSubmit}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const dialog = await screen.findByRole("dialog");
    const propose = () => within(dialog).getByRole("button", { name: en.pipelines.propose });
    expectDenied(propose(), en.pipelines.test.gate);
    // Visible beside the button as a live region, and on the button as its description.
    expect(within(dialog).getAllByText(en.pipelines.test.gate).length).toBeGreaterThan(0);

    await userEvent.upload(within(dialog).getByLabelText(en.pipelines.test.chooseFile), csvFile());
    await userEvent.click(await within(dialog).findByRole("button", { name: en.pipelines.test.run }));
    await within(dialog).findByText(/All 1 messages map to entities/);
    expect(within(dialog).queryByText(en.pipelines.test.gate)).toBeNull();
    // The mapping's test is green; the manifest's own check is the second gate (AG-62).
    await userEvent.click(within(dialog).getByRole("button", { name: en.form.check }));
    await waitFor(() => expectOpen(propose()));

    // The mapping changed after the test: the verdict no longer describes the editor's text.
    await userEvent.type(within(dialog).getByLabelText(/Bloblang mapping/), "\nroot.x = 1");
    await waitFor(() => expectDenied(propose(), en.pipelines.test.gate));
    // Visible beside the button as a live region, and on the button as its description.
    expect(within(dialog).getAllByText(en.pipelines.test.gate).length).toBeGreaterThan(0);
    expect(onSubmit).not.toHaveBeenCalled();
  });
});

/**
 * The UI contract of the studio (T-1775, UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * The Bloblang box was a hand-made `<textarea>` carrying its own `aria-label` and its own border
 * classes, and the sampled entities were a hand-made `<table>` with a hand-made tick per row;
 * both are the shared controls now, so the label, the focus ring, the header scope and the
 * caption come from one place. These tests hold that, plus axe and the four locales.
 */
describe("the pipeline studio against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  function renderAlone(initial?: PipelineForm, dataSources?: Manifest[]) {
    const onForm = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const view = render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness initial={initial} onForm={onForm} dataSources={dataSources} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    return { container: view.container, onForm, user: userEvent.setup() };
  }

  it("has no axe violation with the studio open on a file source", async () => {
    const { container } = renderAlone(undefined, [HTTP_SOURCE]);

    await expectNoViolations(container);
  });

  it("names the Bloblang box through its Field and keeps what is typed in the draft", async () => {
    const { onForm, user } = renderAlone({ compute: { kind: "bloblang", bloblang: "root = this" } });

    await user.click(screen.getByTestId("flow-node-compute"));
    const box = screen.getByTestId("flow-bloblang");
    expect(box.tagName).toBe("TEXTAREA");
    // One name, from the Field's label, rather than a hand-written `aria-label` beside it.
    expect(box).toHaveAccessibleName(en.pipelines.flow.nodeBloblang);
    expect(screen.getByLabelText(en.pipelines.flow.nodeBloblang)).toBe(box);

    box.focus();
    expect(document.activeElement).toBe(box);
    await user.type(box, "\nroot.x = 1");
    expect(onForm).toHaveBeenCalled();
    const last = onForm.mock.calls[onForm.mock.calls.length - 1][0] as PipelineForm;
    expect(last.compute?.bloblang).toContain("root.x = 1");
  });

  it("has no axe violation with the compute node's editor open", async () => {
    const { container, user } = renderAlone({
      compute: { kind: "bloblang", bloblang: "root = this" },
    });

    await user.click(screen.getByTestId("flow-node-compute"));
    await screen.findByTestId("flow-bloblang");
    await expectNoViolations(container);
  });

  it("reaches the controls of the source section by keyboard in the order they are read", async () => {
    const { user } = renderAlone(undefined, [HTTP_SOURCE]);

    const source = screen
      .getByLabelText(en.pipelines.studio.sourceKind)
      .closest("section") as HTMLElement;
    await expectTabOrder(user, source);
  });

  it.each(SUPPORTED_LOCALES)("writes the studio in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = renderAlone(undefined, [HTTP_SOURCE]);

    expect(screen.getByLabelText(i18n.t("pipelines.studio.sourceKind"))).toBeInTheDocument();
    expectNoRawKeys(container);
  });
});
