// T-7706: PipelineFlow canvas gaps: route step wires, undo/redo with no history, debug-tap drop with a laid-out canvas, list-view shifts, route condition click, compute removal.
import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import {
  PipelineFlow,
  addStep,
  paintOf,
  removeRouteNode,
  setComputeKind,
  toFlow,
} from "../src/pages/pipelines/PipelineFlow";
import type { FlowNodeId } from "../src/pages/pipelines/PipelineFlow";
import type { Trace } from "../src/pages/pipelines/PipelineTest";

const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:mobility:ep-bikes";
const base: PipelineForm = {
  name: "bikes",
  class: "auto",
  source: { dataSourceRef: "feed-bikes" },
  compute: { kind: "bloblang", bloblang: "root = this" },
  targetEndpoint: TARGET,
};
const routed: PipelineForm = {
  ...base,
  routes: [
    {
      name: "hot",
      when: "this.t > 30",
      steps: [
        { processor: { log: {} } },
        { processor: { log: {} } },
      ],
      outputs: [{ targetEndpoint: TARGET, mode: "upsert" }],
    },
  ],
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("pure flow helpers", () => {
  it("chains the steps of one route neighbour to neighbour", () => {
    const { edges } = toFlow(routed);
    expect(edges).toContainEqual({ from: "route-0-step-0", to: "route-0-step-1" });
    expect(edges).toContainEqual({ from: "route-0-step-1", to: "route-0-output-0" });
  });

  it("leaves the form alone when the route node names no route", () => {
    expect(removeRouteNode(routed, "route-7-step-0")).toBe(routed);
    expect(removeRouteNode(routed, "source")).toBe(routed);
  });

  it("puts a processor behind a head step that is not the last one", () => {
    const form: PipelineForm = {
      class: "auto",
      processors: [{ step: { processor: { log: {} } } }, { step: { processor: { jq: { query: "." } } } }],
    };
    const added = addStep(form, "step-0", "unarchive");
    expect(added?.id).toBe("step-1");
    expect(added?.form.processors?.map((entry) => Object.keys(entry.step.processor as object)[0])).toEqual([
      "log",
      "unarchive",
      "jq",
    ]);
  });

  it("keeps the bloblang text when the compute kind is set to bloblang again, and drops it for another kind", () => {
    expect(setComputeKind(base, "bloblang").compute).toEqual({ kind: "bloblang", bloblang: "root = this" });
    expect(setComputeKind(base, "mapping").compute).toEqual({ kind: "mapping" });
  });

  it("paints only the first output error of a trace", () => {
    const trace: Trace = {
      input: { events: 1, bytes: 1 },
      mapping: [],
      validation: [],
      errors: [
        { stage: "output", message: "first" },
        { stage: "validation", message: "second" },
      ],
    };
    const paint = paintOf(trace, toFlow(base).nodes);
    expect(paint.output).toMatchObject({ state: "error", error: "first" });
  });
});

function Harness({
  start,
  onTap,
  onOpen,
}: {
  start: PipelineForm;
  onTap?: (key: string) => void;
  onOpen?: (id: FlowNodeId) => void;
}) {
  const [form, setForm] = useState<PipelineForm>(start);
  const [selected, setSelected] = useState<FlowNodeId | null>(null);
  return (
    <I18nextProvider i18n={i18n}>
      <PipelineFlow
        form={form}
        onChange={setForm}
        trace={null}
        selected={selected}
        onSelect={setSelected}
        onOpen={onOpen}
        onTap={onTap}
        dataSources={[]}
        endpoints={[]}
      />
      <pre data-testid="form-json">{JSON.stringify(form)}</pre>
    </I18nextProvider>
  );
}

const formJson = (): PipelineForm => JSON.parse(screen.getByTestId("form-json").textContent ?? "{}") as PipelineForm;

describe("PipelineFlow interactions", () => {
  it("does nothing on ctrl+z or ctrl+shift+z while there is no history", () => {
    render(<Harness start={base} />);
    const before = screen.getByTestId("form-json").textContent;
    fireEvent.keyDown(screen.getByTestId("flow-canvas"), { key: "z", ctrlKey: true });
    fireEvent.keyDown(screen.getByTestId("flow-canvas"), { key: "z", ctrlKey: true, shiftKey: true });
    expect(screen.getByTestId("form-json").textContent).toBe(before);
  });

  it("removes the compute step from the palette's remove button", async () => {
    const user = userEvent.setup();
    render(<Harness start={base} />);
    await user.click(screen.getByTestId("palette-bloblang"));
    expect(formJson().compute?.kind).toBe("bloblang");
    await user.click(screen.getByRole("button", { name: en.pipelines.flow.remove }));
    expect(formJson().compute).toBeUndefined();
  });

  it("starts a drag of a palette kind and of the debug tool with the kind in the transfer", () => {
    const onTap = vi.fn();
    render(<Harness start={base} onTap={onTap} />);
    const setData = vi.fn();
    fireEvent.dragStart(screen.getByTestId("palette-bloblang"), { dataTransfer: { setData } });
    expect(setData).toHaveBeenCalledWith("text/plain", "bloblang");
    fireEvent.click(screen.getByTestId("flow-node-compute"));
    fireEvent.dragStart(screen.getByTestId("palette-debug"), { dataTransfer: { setData } });
    expect(setData).toHaveBeenCalledWith("text/plain", "debug");
  });

  it("taps the wire nearest to a debug drop on a laid-out canvas", () => {
    const onTap = vi.fn();
    render(<Harness start={base} onTap={onTap} />);
    const canvas = screen.getByTestId("flow-canvas");
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({
      left: 0, top: 0, width: 520, height: 300, right: 520, bottom: 300, x: 0, y: 0, toJSON: () => ({}),
    });
    fireEvent.drop(canvas, { clientX: 5, clientY: 5, dataTransfer: { getData: () => "debug" } });
    expect(onTap).toHaveBeenCalledTimes(1);
    expect(onTap.mock.calls[0][0]).toEqual(expect.any(String));
  });

  it("moves a step up from the list view", async () => {
    const user = userEvent.setup();
    render(
      <Harness
        start={{
          ...base,
          processors: [{ step: { processor: { log: {} } } }, { step: { processor: { jq: { query: "." } } } }],
        }}
      />,
    );
    await user.click(screen.getByRole("button", { name: en.pipelines.flow.asList }));
    const moveUp = screen.getAllByRole("button", { name: /Move up/ });
    await user.click(moveUp[1]);
    expect(formJson().processors?.map((entry) => Object.keys(entry.step.processor as object)[0])).toEqual(["jq", "log"]);
  });

  it("opens the route's output when its condition text is clicked", async () => {
    const user = userEvent.setup();
    const onOpen = vi.fn();
    render(<Harness start={routed} onOpen={onOpen} />);
    await user.click(screen.getByTestId("flow-route-when-0"));
    expect(onOpen).toHaveBeenCalledWith("route-0-output-0");
  });
});
