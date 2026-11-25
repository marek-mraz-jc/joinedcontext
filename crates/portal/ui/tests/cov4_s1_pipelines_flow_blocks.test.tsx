// T-7706: PipelineFlow blocks: the entity type picked in a source query and in an output, a step form submitted, a processor dragged from the search list and the tier list, the empty-form node removal.
import { useState } from "react";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import type { OutputForm, PipelineForm, SourceForm } from "../src/pages/pipelines/PipelineEditor";
import { OutputBlock, PipelineFlow, SourceBlock, StepBlock } from "../src/pages/pipelines/PipelineFlow";
import type { FlowNodeId } from "../src/pages/pipelines/PipelineFlow";

const MODELS = [
  { name: "mobility", level: "project", project: "helsinki", version: "0.3.0", lifecycle: "draft", classes: ["BikeHireDockingStation", "Road"] },
];

function stubFetch() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" || input instanceof URL ? String(input) : input.url, window.location.origin);
      if (url.pathname === "/api/v1/organization/datamodels") {
        return Response.json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: MODELS, smartDataModels: [] });
      }
      return new Response("not found", { status: 404 });
    }),
  );
}

function wrap(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
}

const endpoint: Manifest = {
  apiVersion: "joinedcontext.com/v1alpha2",
  kind: "Endpoint",
  metadata: { name: "ep-bikes", namespace: "helsinki" },
  spec: {},
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
  stubFetch();
});
afterEach(() => vi.unstubAllGlobals());

describe("entity type pickers inside the blocks", () => {
  it("sets the query type of an endpoint source from the picker", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const source: SourceForm = { endpointRef: "ep-bikes", query: { attrs: ["a"] } };
    wrap(
      <SourceBlock project="helsinki" source={source} dataSources={[]} endpoints={[endpoint]} locale="en" onChange={onChange} />,
    );
    await user.click(screen.getByRole("combobox", { name: en.pipelines.field.queryType }));
    await user.click(await screen.findByRole("option", { name: /Road/ }));
    expect(onChange).toHaveBeenCalledWith({ endpointRef: "ep-bikes", query: { attrs: ["a"], type: "Road" } });
  });

  it("sets the entity type of an output from the picker", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const output: OutputForm = { mode: "upsert" };
    wrap(<OutputBlock project="helsinki" output={output} targets={[]} locale="en" onChange={onChange} />);
    await user.click(screen.getByRole("combobox", { name: en.pipelines.field.outputType }));
    await user.click(await screen.findByRole("option", { name: /BikeHireDockingStation/ }));
    expect(onChange).toHaveBeenCalledWith({ mode: "upsert", type: "BikeHireDockingStation" });
  });
});

describe("StepBlock form", () => {
  it("takes a submit of its form without changing the step", async () => {
    const onChange = vi.fn();
    wrap(
      <StepBlock entry={{ step: { processor: { log: { message: "hi" } } } }} onChange={onChange} onRemove={() => undefined} />,
    );
    const panel = await screen.findByTestId("flow-step-form");
    const form = panel.querySelector("form");
    expect(form).not.toBeNull();
    fireEvent.submit(form as HTMLFormElement);
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByTestId("flow-step-form")).toBeTruthy();
  });
});

function Harness({ start }: { start?: PipelineForm }) {
  const [form, setForm] = useState<PipelineForm | undefined>(start);
  const [selected, setSelected] = useState<FlowNodeId | null>(null);
  return (
    <I18nextProvider i18n={i18n}>
      <PipelineFlow
        form={form}
        onChange={setForm}
        trace={null}
        selected={selected}
        onSelect={setSelected}
        dataSources={[]}
        endpoints={[]}
      />
      <pre data-testid="form-json">{JSON.stringify(form ?? null)}</pre>
    </I18nextProvider>
  );
}

describe("PipelineFlow palette lists and canvas edges", () => {
  it("starts a drag of a processor from the search results and from the tier list", async () => {
    const user = userEvent.setup();
    render(<Harness start={{ class: "auto" }} />);
    const setData = vi.fn();
    await user.type(screen.getByRole("searchbox"), "unarchive");
    fireEvent.dragStart(await screen.findByTestId("palette-found-unarchive"), { dataTransfer: { setData } });
    expect(setData).toHaveBeenCalledWith("text/plain", "processor:unarchive");
    setData.mockClear();
    await user.clear(screen.getByRole("searchbox"));
    // The tier lists sit in closed disclosures: open each one before the button is on screen.
    for (const summary of document.querySelectorAll("summary")) await user.click(summary);
    fireEvent.dragStart(await screen.findByTestId("palette-processor-unarchive"), { dataTransfer: { setData } });
    expect(setData).toHaveBeenCalledWith("text/plain", "processor:unarchive");
  });

  it("ignores ctrl+z typed in the search box, so the text field keeps its own undo", async () => {
    const user = userEvent.setup();
    render(<Harness start={{ class: "auto", compute: { kind: "bloblang", bloblang: "root = this" } }} />);
    await user.click(screen.getByTestId("palette-source"));
    const before = screen.getByTestId("form-json").textContent;
    fireEvent.keyDown(screen.getByRole("searchbox"), { key: "z", ctrlKey: true });
    expect(screen.getByTestId("form-json").textContent).toBe(before);
    fireEvent.keyDown(screen.getByTestId("flow-canvas"), { key: "z", ctrlKey: true });
    expect(screen.getByTestId("form-json").textContent).not.toBe(before);
  });

  it("removes the compute node from the list view and leaves the rest of the form", async () => {
    const user = userEvent.setup();
    render(<Harness start={{ class: "auto", name: "x", compute: { kind: "bloblang", bloblang: "root = this" } }} />);
    await user.click(screen.getByRole("button", { name: en.pipelines.flow.asList }));
    await user.click(within(screen.getByTestId("flow-list")).getByRole("button", { name: new RegExp(`^${en.pipelines.flow.removeNode}`) }));
    expect(JSON.parse(screen.getByTestId("form-json").textContent ?? "null")).toEqual({ class: "auto", name: "x" });
  });

  it("taps the wire into the selected node when a debug drop has no layout to read", () => {
    const onTap = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={{ class: "auto", compute: { kind: "bloblang", bloblang: "root = this" } }}
          onChange={() => undefined}
          trace={null}
          selected="compute"
          onSelect={() => undefined}
          onTap={onTap}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>,
    );
    fireEvent.drop(screen.getByTestId("flow-canvas"), { dataTransfer: { getData: () => "debug" } });
    expect(onTap).toHaveBeenCalledTimes(1);
  });

  it("follows a wire being drawn with the pointer on a laid-out canvas", async () => {
    render(<Harness start={{ class: "auto", compute: { kind: "bloblang", bloblang: "root = this" } }} />);
    const canvas = screen.getByTestId("flow-canvas");
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({
      left: 0, top: 0, width: 520, height: 300, right: 520, bottom: 300, x: 0, y: 0, toJSON: () => ({}),
    });
    fireEvent.pointerDown(screen.getByTestId("flow-port-out-source"));
    fireEvent.pointerMove(canvas, { clientX: 100, clientY: 50 });
    // A zero-sized canvas has nothing to map the pointer onto, so the wire stays where it is.
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({
      left: 0, top: 0, width: 0, height: 0, right: 0, bottom: 0, x: 0, y: 0, toJSON: () => ({}),
    });
    fireEvent.pointerMove(canvas, { clientX: 200, clientY: 80 });
    await waitFor(() => expect(canvas.querySelector("line.stroke-primary-soft-fg")).not.toBeNull());
    fireEvent.pointerUp(canvas);
    expect(canvas.querySelector("line.pointer-events-none.stroke-primary-soft-fg")).toBeNull();
  });
});
