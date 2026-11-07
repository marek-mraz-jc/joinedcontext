/**
 * T-3221: the pipeline studio as a graph (PL-56, PL-68). A wire drawn from a port to a port sets
 * the order and a wire the runner cannot run is refused in words; every graph edit can be undone
 * and redone; a node says what it lacks before any test; a double-click or Enter opens its form;
 * the canvas zooms; the palette is searchable; and a list view builds the same pipeline by
 * keyboard.
 */
import { useState } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { PipelineFlow } from "../src/pages/pipelines/PipelineFlow";
import type { FlowNodeId } from "../src/pages/pipelines/PipelineFlow";
import { laneOf } from "../src/pages/pipelines/pipelineGraph";

const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:mobility:ep-bikes";
const flow = en.pipelines.flow;

const THREE: PipelineForm = {
  name: "bikes",
  class: "auto",
  source: { dataSourceRef: "feed-bikes" },
  processors: [{ step: { processor: { log: { message: "in" } } } }, { step: { processor: { jq: {} } } }],
  compute: { kind: "bloblang", bloblang: "root = this" },
  targetEndpoint: TARGET,
};

/** The flow with its form held as the studio holds it, so undo sees what it changed. */
function Held({ initial, seen, onOpen }: { initial: PipelineForm; seen: (form: PipelineForm) => void; onOpen?: (id: FlowNodeId) => void }) {
  const [form, setForm] = useState(initial);
  const [selected, setSelected] = useState<FlowNodeId | null>(null);
  return (
    <PipelineFlow
      form={form}
      onChange={(next) => {
        seen(next);
        setForm(next);
      }}
      trace={null}
      selected={selected}
      onSelect={setSelected}
      onOpen={onOpen}
      dataSources={[]}
      endpoints={[]}
    />
  );
}

function held(initial: PipelineForm, onOpen?: (id: FlowNodeId) => void) {
  const forms: PipelineForm[] = [];
  render(
    <I18nextProvider i18n={i18n}>
      <Held initial={initial} seen={(form) => forms.push(form)} onOpen={onOpen} />
    </I18nextProvider>,
  );
  return { last: () => forms[forms.length - 1], forms };
}

/** The lane by what each node runs: a step is named by its index, which a move changes. */
const run = (form: PipelineForm) =>
  laneOf(form).map((id) =>
    id === "compute" ? "compute" : Object.keys(form.processors?.[Number(id.slice(5))]?.step.processor as object)[0],
  );

const drawWire = (from: string, to: string) => {
  fireEvent.pointerDown(screen.getByTestId(`flow-port-out-${from}`));
  fireEvent.pointerUp(screen.getByTestId(`flow-port-in-${to}`));
};

describe("the pipeline graph", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("moves a step behind the node a wire starts from, and undoes and redoes it", async () => {
    const user = userEvent.setup();
    const { last } = held(THREE);
    expect(run(THREE)).toEqual(["log", "jq", "compute"]);

    drawWire("compute", "step-0");
    expect(run(last())).toEqual(["jq", "compute", "log"]);
    expect(screen.getByTestId("flow-said")).toHaveTextContent(flow.wire.moved);

    await user.click(screen.getByRole("button", { name: flow.undo }));
    expect(run(last())).toEqual(["log", "jq", "compute"]);
    expect(screen.getByRole("button", { name: flow.undo })).toHaveAttribute("aria-disabled", "true");

    await user.click(screen.getByRole("button", { name: flow.redo }));
    expect(run(last())).toEqual(["jq", "compute", "log"]);
    expect(screen.getByRole("button", { name: flow.redo })).toHaveAttribute("aria-disabled", "true");

    // The keys do the same from anywhere on the graph that is not a text box.
    fireEvent.keyDown(screen.getByTestId("flow-node-source"), { key: "z", ctrlKey: true });
    expect(run(last())).toEqual(["log", "jq", "compute"]);
    fireEvent.keyDown(screen.getByTestId("flow-node-source"), { key: "Z", ctrlKey: true, shiftKey: true });
    expect(run(last())).toEqual(["jq", "compute", "log"]);
  });

  it("refuses a wire the runner cannot run with the reason, and changes nothing", () => {
    const { forms } = held(THREE);
    drawWire("step-0", "output");
    expect(screen.getByTestId("flow-said")).toHaveTextContent(flow.wire.middleIntoOutput);
    // A source has no input port, and an output no output port, to start a wire the wrong way.
    expect(screen.queryByTestId("flow-port-in-source")).toBeNull();
    expect(screen.queryByTestId("flow-port-out-output")).toBeNull();
    drawWire("compute", "output");
    expect(screen.getByTestId("flow-said")).toHaveTextContent(flow.wire.already);
    expect(forms).toEqual([]);
  });

  // T-4271: a source has no input port, so a wire let go on it used to fall to the canvas and
  // vanish without a word.
  it("says a source takes no wire when one is let go on it, and changes nothing", () => {
    const { forms } = held(THREE);
    fireEvent.pointerDown(screen.getByTestId("flow-port-out-compute"));
    fireEvent.pointerUp(screen.getByTestId("flow-node-source"));
    expect(screen.getByTestId("flow-said")).toHaveTextContent(flow.wire.intoSource);
    expect(forms).toEqual([]);
  });

  it("ends a wire let go on a node's body as on its input port", () => {
    const { last } = held(THREE);
    fireEvent.pointerDown(screen.getByTestId("flow-port-out-compute"));
    fireEvent.pointerUp(screen.getByTestId("flow-node-step-0"));
    expect(run(last())).toEqual(["jq", "compute", "log"]);
    expect(screen.getByTestId("flow-said")).toHaveTextContent(flow.wire.moved);
  });

  it("drops a wire let go on the canvas between nodes", () => {
    const { forms } = held(THREE);
    fireEvent.pointerDown(screen.getByTestId("flow-port-out-compute"));
    fireEvent.pointerUp(screen.getByTestId("flow-canvas"));
    fireEvent.pointerUp(screen.getByTestId("flow-port-in-step-0"));
    expect(forms).toEqual([]);
  });

  it("says what a node lacks before any test, in its name and on the canvas", () => {
    held({ class: "auto", compute: { kind: "bloblang" } });
    expect(screen.getByTestId("flow-node-state-source")).toHaveTextContent(flow.state.missing.source);
    expect(screen.getByTestId("flow-node-state-compute")).toHaveTextContent(flow.state.missing.mapping);
    expect(screen.getByTestId("flow-node-output")).toHaveAccessibleName(
      `${flow.node.output}: output, ${flow.state.missing.output}`,
    );
  });

  it("opens a node's form on a double-click and on Enter", () => {
    const onOpen = vi.fn();
    held(THREE, onOpen);
    fireEvent.doubleClick(screen.getByTestId("flow-node-step-1"));
    expect(onOpen).toHaveBeenLastCalledWith("step-1");
    fireEvent.keyDown(screen.getByTestId("flow-node-compute"), { key: "Enter" });
    expect(onOpen).toHaveBeenLastCalledWith("compute");
  });

  it("zooms in and out within bounds, and fits back to the width", async () => {
    const user = userEvent.setup();
    held(THREE);
    const svg = screen.getByTestId("flow-canvas");
    expect(screen.getByRole("button", { name: flow.fit })).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("button", { name: flow.zoomIn }));
    expect(screen.getByText("125 %")).toBeInTheDocument();
    const wide = Number(svg.getAttribute("width"));
    await user.click(screen.getByRole("button", { name: flow.zoomOut }));
    await user.click(screen.getByRole("button", { name: flow.zoomOut }));
    expect(Number(svg.getAttribute("width"))).toBeLessThan(wide);
    await user.click(screen.getByRole("button", { name: flow.zoomOut }));
    expect(screen.getByText("50 %")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: flow.zoomOut })).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("button", { name: flow.fit }));
    expect(svg).not.toHaveAttribute("width");
    expect(screen.getByText(flow.fitted)).toBeInTheDocument();
  });

  it("finds a processor by name or by what it does, and says when none matches", async () => {
    const user = userEvent.setup();
    const { last } = held(THREE);
    const search = screen.getByLabelText(flow.search);
    await user.type(search, "dedupe");
    expect(screen.getByTestId("palette-processors")).toHaveAttribute("hidden");
    await user.click(screen.getByTestId("palette-found-dedupe"));
    expect(laneOf(last())).toHaveLength(4);
    await user.clear(search);
    await user.type(search, "zzzz-nothing");
    expect(screen.getByText(flow.searchNone.replace("{query}", "zzzz-nothing"))).toBeInTheDocument();
    await user.clear(search);
    expect(screen.getByTestId("palette-processors")).not.toHaveAttribute("hidden");
  });

  it("builds the same order from the list view by keyboard, and removes a node there", async () => {
    const user = userEvent.setup();
    const onOpen = vi.fn();
    const { last } = held(THREE, onOpen);
    await user.click(screen.getByRole("button", { name: flow.asList }));
    expect(screen.getByTestId("flow-canvas").closest("[hidden]")).not.toBeNull();
    const list = screen.getByRole("list", { name: flow.listLabel });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(5);

    // The first step cannot go up; moving it down swaps it with the second.
    const first = within(items[1]);
    expect(first.getByRole("button", { name: /^Move up/ })).toHaveAttribute("aria-disabled", "true");
    await user.click(first.getByRole("button", { name: /^Move down/ }));
    expect(run(last())).toEqual(["jq", "log", "compute"]);

    await user.click(within(list).getAllByRole("button", { name: /^Open/ })[0]);
    expect(onOpen).toHaveBeenLastCalledWith("source");
    // The source and the output are what the pipeline is: the list offers no Remove for them.
    expect(within(within(list).getAllByRole("listitem")[0]).queryByRole("button", { name: /^Remove/ })).toBeNull();
    await user.click(within(within(list).getAllByRole("listitem")[3]).getByRole("button", { name: /^Remove/ }));
    expect(last().compute).toBeUndefined();
  });
});
