/**
 * T-1126, T-1127, T-1128: one lane, sources on the left (PL-52, PL-53, PL-56). The palette lists
 * what the pinned runner ships by category, a processor goes in behind a chosen node, the selected
 * step shows its own Bento block as YAML, a pipeline reads several sources that the runner merges,
 * and what is removed on the canvas is removed in the manifest.
 */
import { useState } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { toEnvelope, toForm } from "../src/pages/pipelines/PipelineEditor";
import {
  PROCESSOR_GROUPS,
  PipelineFlow,
  addSource,
  addStep,
  paintOf,
  plainSummary,
  removeSource,
  removeStep,
  samplesOf,
  toFlow,
} from "../src/pages/pipelines/PipelineFlow";
import { PipelineStudio } from "../src/pages/pipelines/PipelineStudio";
import type { Trace } from "../src/pages/pipelines/PipelineTest";

vi.mock("../src/branding", () => ({
  useBranding: () => ({ orgDomain: "hel.fi" }),
}));

const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:mobility:ep-bikes";

const withCompute: PipelineForm = {
  name: "bikes",
  class: "auto",
  source: { dataSourceRef: "feed-bikes" },
  compute: { kind: "bloblang", bloblang: "root = this" },
  targetEndpoint: TARGET,
};

const pipeline = (steps: unknown[]): Manifest => ({
  apiVersion: "joinedcontext.com/v1alpha2",
  kind: "Pipeline",
  metadata: { name: "bikes", namespace: "helsinki" },
  spec: {
    class: "auto",
    sources: [{ dataSourceRef: { kind: "DataSource", name: "feed-bikes" } }],
    steps,
    outputs: [{ targetEndpoint: TARGET }],
  },
});

const stepsOf = (manifest: Manifest) => (manifest.spec as { steps?: unknown[] }).steps;

describe("the lane's steps", () => {
  it("draws the steps in the order they run, the compute step among them, neighbour to neighbour", () => {
    const form = toForm(
      pipeline([
        { processor: { unarchive: { format: "json_array" } } },
        { kind: "bloblang", bloblang: "root = this" },
        { processor: { jq: { query: ".id" } } },
      ]),
    );
    const { nodes, edges } = toFlow(form);
    expect(nodes.map((node) => `${node.id}:${node.kind}`)).toEqual([
      "source:datasource",
      "step-0:unarchive",
      "compute:bloblang",
      "step-1:jq",
      "output:output",
    ]);
    expect(nodes[1].summary).toBe("format");
    expect(edges).toEqual([
      { from: "source", to: "step-0" },
      { from: "step-0", to: "compute" },
      { from: "compute", to: "step-1" },
      { from: "step-1", to: "output" },
    ]);
  });

  it("puts a processor behind the node it was asked behind", () => {
    const head = addStep(withCompute, "source", "unarchive");
    expect(head?.id).toBe("step-0");
    expect(head?.form.processors).toEqual([{ step: { processor: { unarchive: {} } } }]);

    const behindCompute = addStep(head?.form, "compute", "jq");
    expect(behindCompute?.id).toBe("step-1");
    const tail = addStep(behindCompute?.form, null, "log");
    expect(tail?.id).toBe("step-2");
    const between = addStep(tail?.form, "step-1", "dedupe");
    expect(between?.id).toBe("step-2");
    const onOutput = addStep(between?.form, "output", "noop");

    expect(toFlow(onOutput?.form).nodes.map((node) => node.kind)).toEqual([
      "datasource",
      "unarchive",
      "bloblang",
      "jq",
      "dedupe",
      "log",
      "noop",
      "output",
    ]);
  });

  it("keeps the order in a lane without a compute step, and after one was taken out", () => {
    const bare: PipelineForm = { ...withCompute, compute: undefined };
    const one = addStep(bare, null, "unarchive");
    const two = addStep(one?.form, null, "jq");
    expect(two?.form.processors?.map((entry) => entry.after)).toEqual([undefined, undefined]);

    // The compute step is gone and a step still says it ran after it: the tail stays the tail.
    const orphaned: PipelineForm = {
      ...bare,
      processors: [{ step: { processor: { jq: {} } }, after: true }],
    };
    const added = addStep(orphaned, null, "log");
    expect(toFlow(added?.form).nodes.map((node) => node.kind)).toEqual([
      "datasource",
      "jq",
      "log",
      "output",
    ]);
  });

  it("starts a mapping as text and a composition as a list, which is what the runner lints green", () => {
    expect(addStep(undefined, null, "mapping")?.form.processors?.[0].step).toEqual({
      processor: { mapping: "root = this" },
    });
    expect(addStep(undefined, null, "catch")?.form.processors?.[0].step).toEqual({
      processor: { catch: [] },
    });
  });

  it("adds nothing for a name the runner does not ship, programs in the shared runner included", () => {
    for (const name of ["command", "subprocess", "", "__proto__", "JQ"]) {
      expect(addStep(withCompute, null, name)).toBeUndefined();
    }
    const names = PROCESSOR_GROUPS.flatMap(({ processors }) => processors.map(({ name }) => name));
    expect(names).not.toContain("command");
    expect(names).not.toContain("subprocess");
    expect(new Set(names).size).toBe(names.length);
    expect(PROCESSOR_GROUPS.every(({ processors }) => processors.length > 0)).toBe(true);
  });

  it("says a summary in words, without the runner's Markdown targets", () => {
    expect(plainSummary("offers [custom functions](#awk-functions) for it")).toBe(
      "offers custom functions for it",
    );
    expect(plainSummary("no link")).toBe("no link");
  });

  it("writes the steps around the compute step, an empty configuration included", () => {
    const form = addStep(addStep(withCompute, "source", "noop")?.form, "compute", "jq")?.form;
    const written = toEnvelope("helsinki", form as PipelineForm);
    expect(written.apiVersion).toBe("joinedcontext.com/v1alpha2");
    expect(stepsOf(written)).toEqual([
      { processor: { noop: {} } },
      { kind: "bloblang", bloblang: "root = this" },
      { processor: { jq: {} } },
    ]);
    expect(toForm(written).processors).toEqual(form?.processors);
  });

  it("drops the last step from the manifest being edited, not only from the canvas", () => {
    const editing = pipeline([{ processor: { jq: { query: ".id" } } }]);
    const form = toForm(editing);
    expect(form.processors).toHaveLength(1);

    const removed = removeStep(form, 0);
    expect(removed.processors).toEqual([]);
    expect(stepsOf(toEnvelope("helsinki", removed, editing))).toBeUndefined();
    // A form that never carried the lane keeps the steps of the manifest it edits (PL-54).
    expect(stepsOf(toEnvelope("helsinki", { ...form, processors: undefined }, editing))).toEqual([
      { processor: { jq: { query: ".id" } } },
    ]);
    // An index that names no step removes nothing.
    expect(removeStep(form, 7).processors).toEqual(form.processors);
  });

  it("paints a failed mapping on the first step of a lane that has no compute step", () => {
    const form = addStep({ ...withCompute, compute: undefined }, null, "jq")?.form;
    const trace: Trace = {
      input: { events: 2, bytes: 20 },
      mapping: [],
      validation: [],
      errors: [{ stage: "mapping", message: "jq: bad query" }],
    };
    const paint = paintOf(trace, toFlow(form).nodes);
    expect(paint.source.state).toBe("ok");
    expect(paint["step-0"]).toMatchObject({ state: "error", error: "jq: bad query" });
    expect(paint.output.state).toBe("skipped");
    expect(paintOf(null, toFlow(form).nodes)["step-0"].state).toBe("idle");
  });
});

describe("the canvas and the step's block", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify({ items: [] }), {
            headers: { "Content-Type": "application/json" },
          }),
        ),
      ),
    );
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  function canvas(form: PipelineForm, selected: Parameters<typeof PipelineFlow>[0]["selected"]) {
    const onChange = vi.fn();
    const onSelect = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={form}
          onChange={onChange}
          trace={null}
          selected={selected}
          onSelect={onSelect}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>,
    );
    return { onChange, onSelect };
  }

  // The 75 processors sat between the compute palette and the canvas they are dropped on, so a
  // 1280x720 dialog could not show a palette and its drop target at once: scrolling the canvas
  // into view took the palette off the screen, and the e2e drag of a compute kind stopped
  // landing (ci-full run 35507319695, `flow-node-compute` never appeared). Both palettes now
  // touch the canvas — the compute toolbar above it, the processors below.
  it("keeps the canvas between its two palettes, with nothing tall in between", () => {
    canvas(withCompute, null);
    const compute = screen.getByTestId("palette-bloblang");
    const flow = screen.getByTestId("flow-canvas");
    const processors = screen.getByTestId("palette-processors");
    const follows = (a: Element, b: Element) =>
      Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    expect(follows(compute, flow)).toBe(true);
    expect(follows(flow, processors)).toBe(true);
  });

  it("lists every processor under its category with the runner's summary", () => {
    canvas(withCompute, null);
    const palette = screen.getByTestId("palette-processors");
    for (const { category, processors } of PROCESSOR_GROUPS) {
      const list = within(palette).getByRole("list", { name: category, hidden: true });
      expect(within(list).getAllByRole("listitem", { hidden: true })).toHaveLength(
        processors.length,
      );
    }
    const awk = within(palette).getByTestId("palette-processor-awk").closest("li");
    expect(awk).toHaveTextContent("custom functions");
    expect(awk).not.toHaveTextContent("#awk-functions");
  });

  it("puts a clicked processor behind the selected node and selects it", async () => {
    const { onChange, onSelect } = canvas(withCompute, "compute");
    await userEvent.click(screen.getByTestId("palette-processor-jq"));
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ processors: [{ step: { processor: { jq: {} } }, after: true }] }),
    );
    expect(onSelect).toHaveBeenCalledWith("step-0");
  });

  it("takes a dropped processor and refuses a dropped name the runner does not ship", () => {
    const { onChange, onSelect } = canvas(withCompute, "source");
    const drop = (text: string) =>
      fireEvent.drop(screen.getByTestId("flow-canvas"), {
        dataTransfer: { getData: () => text, dropEffect: "" },
      });
    drop("processor:command");
    drop("processor:");
    expect(onChange).not.toHaveBeenCalled();

    drop("processor:unarchive");
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ processors: [{ step: { processor: { unarchive: {} } } }] }),
    );
    expect(onSelect).toHaveBeenCalledWith("step-0");
  });

  it("removes a step with Delete and leaves the source and the output alone", () => {
    const form = addStep(withCompute, "source", "jq")?.form as PipelineForm;
    const { onChange, onSelect } = canvas(form, "step-0");
    fireEvent.keyDown(screen.getByTestId("flow-node-source"), { key: "Delete" });
    fireEvent.keyDown(screen.getByTestId("flow-node-output"), { key: "Backspace" });
    expect(onChange).not.toHaveBeenCalled();

    const node = screen.getByTestId("flow-node-step-0");
    expect(node).toHaveAccessibleName("Step: jq, Configured");
    fireEvent.keyDown(node, { key: "Delete" });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ processors: [] }));
    expect(onSelect).toHaveBeenCalledWith(null);
  });

  function Studio({ initial, seen }: { initial: PipelineForm; seen: (form: PipelineForm) => void }) {
    const [draft, setDraft] = useState(initial);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    return (
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <PipelineStudio
            project="helsinki"
            draft={draft}
            onChange={(form) => {
              seen(form);
              setDraft(form);
            }}
            dataSources={[]}
            endpoints={[]}
            toManifest={vi.fn()}
          />
        </I18nextProvider>
      </QueryClientProvider>
    );
  }

  it("shows the selected step's own block, takes an edit, and says why it keeps the last good one", async () => {
    const seen = vi.fn();
    const initial: PipelineForm = {
      ...withCompute,
      processors: [
        { step: { processor: { jq: { query: ".id" } } } },
        {
          step: {
            processor: {
              http: {
                url: "https://feed.example",
                headers: { Authorization: "${FEED_TOKEN}" },
              },
            },
          },
          after: true,
        },
      ],
    };
    render(<Studio initial={initial} seen={seen} />);

    await userEvent.click(screen.getByTestId("flow-node-step-0"));
    // The form is the first view of a processor the runner documents (T-3088); this is the text.
    await userEvent.click(await screen.findByRole("tab", { name: "YAML" }));
    const block = screen.getByTestId("flow-step-yaml") as HTMLTextAreaElement;
    expect(block.value).toBe("jq:\n  query: .id\n");
    // Its own block and nothing of a neighbour's (PL-56).
    expect(block.value).not.toContain("feed.example");
    expect(block).toHaveAccessibleName("This step's Bento block");
    expect(block).toHaveAccessibleDescription(/A secret stays a \$\{VAR\} reference\./);

    fireEvent.change(block, { target: { value: "jq:\n  query: .name\n" } });
    expect(seen).toHaveBeenLastCalledWith(
      expect.objectContaining({
        processors: [{ step: { processor: { jq: { query: ".name" } } } }, initial.processors?.[1]],
      }),
    );

    const calls = seen.mock.calls.length;
    for (const [text, said] of [
      ["jq: [", /This is not YAML yet/],
      ["jq: {}\nlog: {}\n", /A step is one processor/],
      ["", /A step is one processor/],
      ["- jq", /A step is one processor/],
      ["command:\n  name: rm\n", /ships no processor named "command"/],
    ] as const) {
      fireEvent.change(block, { target: { value: text } });
      expect(screen.getByRole("alert")).toHaveTextContent(said);
      expect(block).toBeInvalid();
    }
    expect(seen.mock.calls.length).toBe(calls);

    fireEvent.change(block, { target: { value: "jq:\n  query: .id\n" } });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(block).toBeValid();
  });

  it("keeps a secret reference a reference in the block it shows", async () => {
    const initial: PipelineForm = {
      ...withCompute,
      processors: [
        {
          step: {
            processor: {
              http: {
                url: "https://feed.example",
                headers: { Authorization: "${FEED_TOKEN}" },
              },
            },
          },
        },
      ],
    };
    render(<Studio initial={initial} seen={vi.fn()} />);
    await userEvent.click(screen.getByTestId("flow-node-step-0"));
    await userEvent.click(await screen.findByRole("tab", { name: "YAML" }));
    expect((screen.getByTestId("flow-step-yaml") as HTMLTextAreaElement).value).toContain(
      "Authorization: ${FEED_TOKEN}",
    );
  });

  it("says at the compute node that only a Bloblang step runs today", async () => {
    render(<Studio initial={{ ...withCompute, compute: { kind: "wasm", module: "m", function: "f" } }} seen={vi.fn()} />);
    await userEvent.click(screen.getByTestId("flow-node-compute"));
    expect(screen.getByTestId("flow-compute-not-run")).toHaveTextContent(
      "The platform does not run a step of this kind yet (Compute module (WebAssembly))",
    );
  });

  it("says nothing of the sort for a Bloblang step", async () => {
    render(<Studio initial={withCompute} seen={vi.fn()} />);
    await userEvent.click(screen.getByTestId("flow-node-compute"));
    expect(screen.queryByTestId("flow-compute-not-run")).not.toBeInTheDocument();
  });

  it("shows a second compute step as it is written and removes it from its own button", async () => {
    const seen = vi.fn();
    const initial: PipelineForm = {
      ...withCompute,
      processors: [{ step: { kind: "bloblang", bloblang: "root.extra = 1" }, after: true }],
    };
    render(<Studio initial={initial} seen={seen} />);
    await userEvent.click(screen.getByTestId("flow-node-step-0"));
    expect(screen.queryByTestId("flow-step-yaml")).not.toBeInTheDocument();
    expect(screen.getByTestId("flow-step-readonly")).toHaveTextContent("root.extra = 1");

    await userEvent.click(screen.getByTestId("flow-step-remove"));
    expect(seen).toHaveBeenLastCalledWith(expect.objectContaining({ processors: [] }));
    expect(screen.queryByTestId("flow-node-editor-step")).not.toBeInTheDocument();
  });
});

describe("a pipeline that reads several sources", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal(
      "fetch",
      vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify({ items: [] }), {
            headers: { "Content-Type": "application/json" },
          }),
        ),
      ),
    );
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("stands every source on the left, each feeding the head of the lane", () => {
    const form: PipelineForm = {
      ...withCompute,
      moreSources: [{ dataSourceRef: "feed-weather" }, { endpointRef: "ep-traffic" }],
    };
    const { nodes, edges } = toFlow(form);
    expect(nodes.map((node) => node.id)).toEqual([
      "source",
      "source-0",
      "source-1",
      "compute",
      "output",
    ]);
    expect(nodes[1].summary).toBe("feed-weather");
    expect(nodes[2].kind).toBe("space");
    // No broker node: the runner merges them, an author does not place it (PL-53).
    expect(nodes.map((node) => node.kind)).not.toContain("broker");
    expect(edges).toEqual([
      { from: "source", to: "compute" },
      { from: "source-0", to: "compute" },
      { from: "source-1", to: "compute" },
      { from: "compute", to: "output" },
    ]);
  });

  it("feeds the first step when there is one, not the compute step behind it", () => {
    const form = addStep({ ...withCompute, moreSources: [{ dataSourceRef: "b" }] }, "source", "jq")
      ?.form as PipelineForm;
    expect(toFlow(form).edges.slice(0, 2)).toEqual([
      { from: "source", to: "step-0" },
      { from: "source-0", to: "step-0" },
    ]);
  });

  it("writes every source into the manifest and reads them all back", () => {
    const added = addSource(withCompute);
    expect(added.id).toBe("source-0");
    const form: PipelineForm = {
      ...added.form,
      moreSources: [{ dataSourceRef: "feed-weather" }],
    };
    const written = toEnvelope("helsinki", form);
    expect((written.spec as { sources?: unknown[] }).sources).toEqual([
      { dataSourceRef: { kind: "DataSource", name: "feed-bikes" } },
      { dataSourceRef: { kind: "DataSource", name: "feed-weather" } },
    ]);
    expect(toForm(written).moreSources).toEqual([{ dataSourceRef: "feed-weather" }]);
    // The first source is still the one the studio's own section reads.
    expect(toForm(written).source).toEqual({ dataSourceRef: "feed-bikes" });
  });

  it("drops a removed source from the manifest and keeps one the form never carried", () => {
    const editing = pipeline([]);
    (editing.spec as { sources: unknown[] }).sources.push({
      dataSourceRef: { kind: "DataSource", name: "feed-weather" },
    });
    const form = toForm(editing);
    expect(form.moreSources).toEqual([{ dataSourceRef: "feed-weather" }]);

    const removed = removeSource(form, 0);
    const written = toEnvelope("helsinki", removed, editing);
    expect((written.spec as { sources?: unknown[] }).sources).toEqual([
      { dataSourceRef: { kind: "DataSource", name: "feed-bikes" } },
    ]);
    // A form that never carried them keeps what the manifest held (PL-54).
    expect(
      (toEnvelope("helsinki", { ...form, moreSources: undefined }, editing).spec as {
        sources?: unknown[];
      }).sources,
    ).toHaveLength(2);
  });

  it("marks every source when the read failed, because the trace never says which one", () => {
    const form: PipelineForm = { ...withCompute, moreSources: [{ dataSourceRef: "feed-weather" }] };
    const { nodes } = toFlow(form);
    const paint = paintOf(
      {
        input: { events: 4, bytes: 40 },
        mapping: [],
        validation: [],
        errors: [{ stage: "input", message: "the feed answered 404 Not Found" }],
      },
      nodes,
    );
    expect(paint.source).toMatchObject({ state: "error", eventsIn: 4 });
    expect(paint["source-0"]).toMatchObject({
      state: "error",
      error: "the feed answered 404 Not Found",
    });
    expect(paint.compute.state).toBe("skipped");
    expect(paint.output.state).toBe("skipped");

    // A read that went through leaves every source green, whatever failed behind them.
    const later = paintOf(
      {
        input: { events: 4, bytes: 40 },
        mapping: [],
        validation: [],
        errors: [{ stage: "mapping", message: "root = this: no" }],
      },
      nodes,
    );
    expect(later.source.state).toBe("ok");
    expect(later["source-0"].state).toBe("ok");
    expect(later.compute.state).toBe("error");
  });

  it("adds a source from the palette, picks what it reads and takes it away again", async () => {
    const seen = vi.fn();
    const dataSources: Manifest[] = [
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "DataSource",
        metadata: { name: "feed-weather", namespace: "helsinki" },
        spec: {},
      },
    ];
    function Host() {
      const [draft, setDraft] = useState<PipelineForm>(withCompute);
      const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      return (
        <QueryClientProvider client={client}>
          <I18nextProvider i18n={i18n}>
            <PipelineStudio
              project="helsinki"
              draft={draft}
              onChange={(form) => {
                seen(form);
                setDraft(form);
              }}
              dataSources={dataSources}
              endpoints={[]}
              toManifest={vi.fn()}
            />
          </I18nextProvider>
        </QueryClientProvider>
      );
    }
    render(<Host />);

    await userEvent.click(screen.getByTestId("palette-source"));
    expect(screen.getByTestId("flow-node-source-0")).toBeInTheDocument();

    const pick = screen.getByTestId("flow-source-ref");
    await userEvent.selectOptions(pick, "DataSource:feed-weather");
    expect(seen).toHaveBeenLastCalledWith(
      expect.objectContaining({ moreSources: [{ dataSourceRef: "feed-weather" }] }),
    );

    await userEvent.click(screen.getByTestId("flow-source-remove"));
    expect(seen).toHaveBeenLastCalledWith(expect.objectContaining({ moreSources: [] }));
    expect(screen.queryByTestId("flow-node-source-0")).not.toBeInTheDocument();
  });

  it("asks an endpoint source what it reads and writes it into the source's query", async () => {
    // A second source on an endpoint had no type, attributes or filter: it read nothing (T-3088).
    // The attributes are picked from the model of the endpoint's space (T-4946), never typed.
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL((input as Request).url ?? String(input), "http://localhost");
        const reply = (body: string, type = "application/json") => new Response(body, { status: 200, headers: { "Content-Type": type } });
        if (url.pathname === "/api/v1/organization/datamodels") {
          const items = [{ project: "helsinki", name: "bikes", space: "mobility", classes: ["BikeHireDockingStation"], level: "project", lifecycle: "published", version: "1.0.0" }];
          return reply(JSON.stringify({ apiVersion: "v1", kind: "OrganizationModels", items, smartDataModels: [] }));
        }
        if (url.pathname === "/api/v1/projects/helsinki/datamodels/bikes/source") {
          return reply(
            "id: https://example.org/bikes\nname: bikes\nclasses:\n  BikeHireDockingStation:\n    slots: [availableBikeNumber, totalSlotNumber]\nslots:\n  availableBikeNumber:\n    range: integer\n  totalSlotNumber:\n    range: integer\n",
            "text/yaml",
          );
        }
        return reply(JSON.stringify({ items: [] }));
      }),
    );
    const seen = vi.fn();
    const endpoints: Manifest[] = [
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Endpoint",
        metadata: { name: "ep-docks", namespace: "helsinki" },
        spec: { contextSpaceRef: "mobility", slug: "docks" },
      },
    ];
    function Host() {
      const [draft, setDraft] = useState<PipelineForm>({
        ...withCompute,
        moreSources: [{ endpointRef: "ep-docks", query: { type: "BikeHireDockingStation" } }],
      });
      const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      return (
        <QueryClientProvider client={client}>
          <I18nextProvider i18n={i18n}>
            <PipelineStudio
              project="helsinki"
              draft={draft}
              onChange={(form) => {
                seen(form);
                setDraft(form);
              }}
              dataSources={[]}
              endpoints={endpoints}
              toManifest={vi.fn()}
            />
          </I18nextProvider>
        </QueryClientProvider>
      );
    }
    render(<Host />);
    await userEvent.click(screen.getByTestId("flow-node-source-0"));
    const query = screen.getByTestId("flow-source-query");
    const attrs = within(query).getByRole("combobox", { name: en.pipelines.field.attrs });
    await userEvent.click(attrs);
    await userEvent.click(await screen.findByRole("option", { name: /availableBikeNumber/ }));
    await userEvent.click(await screen.findByRole("option", { name: /totalSlotNumber/ }));
    await userEvent.type(within(query).getByLabelText(/^Attribute filter/), "availableBikeNumber>0");
    const type = "BikeHireDockingStation";
    expect(seen).toHaveBeenLastCalledWith(
      expect.objectContaining({
        moreSources: [
          {
            endpointRef: "ep-docks",
            query: { type, attrs: ["availableBikeNumber", "totalSlotNumber"], q: "availableBikeNumber>0" },
          },
        ],
      }),
    );
    // Removing both picked names reads every attribute again.
    await userEvent.click(within(query).getByRole("button", { name: /Remove availableBikeNumber/ }));
    await userEvent.click(within(query).getByRole("button", { name: /Remove totalSlotNumber/ }));
    expect(seen).toHaveBeenLastCalledWith(
      expect.objectContaining({ moreSources: [{ endpointRef: "ep-docks", query: { type, q: "availableBikeNumber>0" } }] }),
    );
    vi.unstubAllGlobals();
  });

  it("removes a second source with Delete and never the first one", () => {
    const form: PipelineForm = { ...withCompute, moreSources: [{ dataSourceRef: "feed-weather" }] };
    const onChange = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={form}
          onChange={onChange}
          trace={null}
          selected="source-0"
          onSelect={vi.fn()}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>,
    );
    fireEvent.keyDown(screen.getByTestId("flow-node-source"), { key: "Delete" });
    expect(onChange).not.toHaveBeenCalled();

    fireEvent.keyDown(screen.getByTestId("flow-node-source-0"), { key: "Delete" });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ moreSources: [] }));
  });
});

describe("the test trace painted per step", () => {
  const traceOf = (errors: Trace["errors"]): Trace => ({
    input: { events: 3, bytes: 30 },
    mapping: [],
    validation: [],
    errors,
  });

  /** A lane of three steps around the compute one, in the order the manifest writes them. */
  const laneForm: PipelineForm = {
    ...withCompute,
    processors: [
      { step: { processor: { unarchive: { format: "json_array" } } } },
      { step: { processor: { jq: { query: ".id" } } }, after: true },
    ],
  };

  it("paints the step the error names and greys only what runs behind it", () => {
    const { nodes } = toFlow(laneForm);
    expect(nodes.map((node) => node.id)).toEqual([
      "source",
      "step-0",
      "compute",
      "step-1",
      "output",
    ]);

    const paint = paintOf(traceOf([{ stage: "mapping", step: 1, message: "root = this: no" }]), nodes);
    expect(paint.source.state).toBe("ok");
    expect(paint["step-0"].state).toBe("ok");
    expect(paint.compute).toMatchObject({ state: "error", error: "root = this: no" });
    expect(paint["step-1"].state).toBe("skipped");
    expect(paint.output.state).toBe("skipped");
  });

  it("counts the step by the manifest's own order, the compute step among them", () => {
    const { nodes } = toFlow(laneForm);
    const first = paintOf(traceOf([{ stage: "mapping", step: 0, message: "unarchive: no" }]), nodes);
    expect(first["step-0"]).toMatchObject({ state: "error", error: "unarchive: no" });
    expect(first.compute.state).toBe("skipped");

    const last = paintOf(traceOf([{ stage: "mapping", step: 2, message: "jq: no" }]), nodes);
    expect(last["step-0"].state).toBe("ok");
    expect(last.compute.state).toBe("ok");
    expect(last["step-1"]).toMatchObject({ state: "error", error: "jq: no" });
  });

  it("keeps the first error of a step when the runner reports several", () => {
    const { nodes } = toFlow(laneForm);
    const paint = paintOf(
      traceOf([
        { stage: "mapping", step: 0, message: "the first one" },
        { stage: "mapping", step: 0, message: "the second one" },
      ]),
      nodes,
    );
    expect(paint["step-0"].error).toBe("the first one");
  });

  it("says a mapping error that names no step where it was said before", () => {
    const { nodes } = toFlow(laneForm);
    for (const step of [undefined, null] as const) {
      const paint = paintOf(traceOf([{ stage: "mapping", step, message: "no step" }]), nodes);
      expect(paint.compute).toMatchObject({ state: "error", error: "no step" });
      expect(paint["step-0"].state).toBe("ok");
    }
  });

  it("does not lose an error whose step the lane has no node for", () => {
    const { nodes } = toFlow(laneForm);
    // A trace from a manifest with more steps than the form draws: the number is not a node,
    // and an error that vanished would leave a red lane with nothing said on it.
    const paint = paintOf(traceOf([{ stage: "mapping", step: 9, message: "somewhere behind" }]), nodes);
    expect(Object.values(paint).some((node) => node.error === "somewhere behind")).toBe(true);
    expect(paint.output.state).toBe("skipped");
  });

  it("leaves a lint error where it was: it belongs to the document, not to a step", () => {
    const { nodes } = toFlow(laneForm);
    const paint = paintOf(
      traceOf([{ stage: "lint", line: 12, message: "expected string, got number" }]),
      nodes,
    );
    expect(paint.compute).toMatchObject({ state: "error" });
    expect(paint["step-0"].state).toBe("ok");
  });
});

describe("each node's own sample in a test (T-3089, PL-67)", () => {
  const lane: PipelineForm = {
    ...withCompute,
    processors: [
      { step: { processor: { jq: { query: ".a" } } } },
      { step: { processor: { log: { message: "x" } } }, after: true },
    ],
  };
  const trace = {
    input: { events: 1, bytes: 10, sample: { a: 1 } },
    mapping: [{ id: "urn:ngsi-ld:T:x" }],
    validation: [{ index: 0, ok: true, problems: [] }],
    errors: [],
    stages: [
      { step: 0, reached: 1, sample: 1 },
      { step: 1, reached: 1, sample: { id: "urn:ngsi-ld:T:x" } },
      { step: 2, reached: 1, sample: "logged" },
    ],
  } as unknown as Trace;

  it("shows a step what the step before it made, and what it made itself", () => {
    const { nodes } = toFlow(lane);
    expect(samplesOf(trace, nodes, "source")).toEqual({ in: { a: 1 }, out: { a: 1 } });
    expect(samplesOf(trace, nodes, "step-0")).toEqual({ in: { a: 1 }, out: 1 });
    expect(samplesOf(trace, nodes, "compute")).toEqual({ in: 1, out: { id: "urn:ngsi-ld:T:x" } });
    expect(samplesOf(trace, nodes, "step-1")).toEqual({ in: { id: "urn:ngsi-ld:T:x" }, out: "logged" });
    expect(samplesOf(trace, nodes, "output")).toEqual({ in: "logged", out: { id: "urn:ngsi-ld:T:x" } });
  });

  it("says nothing for a step the messages never reached, and keeps the old view without stages", () => {
    const { nodes } = toFlow(lane);
    const stopped = { ...trace, stages: [{ step: 0, reached: 0 }, { step: 1, reached: 0 }, { step: 2, reached: 0 }] } as Trace;
    expect(samplesOf(stopped, nodes, "compute")).toEqual({ in: undefined, out: undefined });
    const older = { ...trace, stages: undefined } as Trace;
    expect(samplesOf(older, nodes, "compute")).toEqual({ in: { a: 1 }, out: { id: "urn:ngsi-ld:T:x" } });
    expect(samplesOf(older, nodes, "step-1")).toEqual({ in: { id: "urn:ngsi-ld:T:x" }, out: { id: "urn:ngsi-ld:T:x" } });
  });
});
