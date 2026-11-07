/**
 * T-3222: debugging on the graph. Counts stand on the wires, a wire opens the node it enters, a
 * Debug node taps a wire and the sidebar shows what crossed it per run, filterable by node; a
 * failing node shows its error and the input that broke it, and reruns alone on that input; step
 * mode runs one message of the sample at a time. Every run is the dry-run test.
 */
import { useState } from "react";
import type { JSX } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Trace } from "../src/api/pipelineTest";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";

const testPipeline = vi.fn();
vi.mock("../src/api/pipelineTest", async (real) => ({
  ...(await real<typeof import("../src/api/pipelineTest")>()),
  testPipeline: (...args: unknown[]) => testPipeline(...args),
}));

const { PipelineFlow, toFlow } = await import("../src/pages/pipelines/PipelineFlow");
const { DebugSidebar, NodeDebug } = await import("../src/pages/pipelines/DebugPanel");
const { PipelineTest } = await import("../src/pages/pipelines/PipelineTest");
type FlowNodeId = import("../src/pages/pipelines/PipelineFlow").FlowNodeId;
type DebugEntry = import("../src/pages/pipelines/DebugPanel").DebugEntry;

const debug = en.pipelines.debug;

const FORM: PipelineForm = {
  name: "air",
  class: "auto",
  source: { dataSourceRef: "feed" },
  processors: [{ step: { processor: { log: {} } } }],
  compute: { kind: "bloblang", bloblang: 'root = this\nroot.type = "AirQualityObserved"' },
  targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:air:ep",
};

/** The compute step (lane index 1) breaks on the second station. */
const BROKEN: Trace = {
  input: { events: 2, bytes: 40, sample: { id: "s1", pm10: 4 } },
  mapping: [],
  validation: [],
  errors: [{ stage: "mapping", step: 1, message: "expected number, got string" }],
  stages: [
    { step: 0, reached: 2, sample: { id: "s1", pm10: "many" } },
    { step: 1, reached: 2 },
  ],
};

const GREEN: Trace = {
  input: { events: 1, bytes: 20, sample: { id: "s1", pm10: 4 } },
  mapping: [{ id: "s1", type: "AirQualityObserved" }],
  validation: [{ index: 0, ok: true, problems: [] }],
  errors: [],
  stages: [
    { step: 0, reached: 1, sample: { id: "s1", pm10: 4 } },
    { step: 1, reached: 1, sample: { id: "s1", pm10: 4, type: "AirQualityObserved" } },
  ],
};

const nameOf = (id: FlowNodeId) => id;

function Graph({ trace, log, onOpen }: { trace: Trace | null; log: DebugEntry[]; onOpen?: (id: FlowNodeId) => void }) {
  const [taps, setTaps] = useState<string[]>([]);
  const [selected, setSelected] = useState<FlowNodeId | null>(null);
  const { nodes, edges } = toFlow(FORM);
  return (
    <>
      <PipelineFlow
        form={FORM}
        onChange={() => undefined}
        trace={trace}
        selected={selected}
        onSelect={setSelected}
        onOpen={onOpen}
        taps={taps}
        onTap={(key) => setTaps((was) => (was.includes(key) ? was.filter((one) => one !== key) : [...was, key]))}
        dataSources={[]}
        endpoints={[]}
      />
      <DebugSidebar log={log} nodes={nodes} edges={edges} taps={taps} nameOf={nameOf} onClear={() => undefined} />
    </>
  );
}

const show = (ui: JSX.Element) => render(<I18nextProvider i18n={i18n}>{ui}</I18nextProvider>);

describe("debugging on the pipeline graph", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    testPipeline.mockReset();
  });

  it("stands a count on every wire after a test, and a wire opens the node it enters", () => {
    const onOpen = vi.fn();
    show(<Graph trace={GREEN} log={[]} onOpen={onOpen} />);
    expect(screen.getByTestId("flow-wire-count-source>step-0")).toHaveTextContent("1");
    expect(screen.getByTestId("flow-wire-count-step-0>compute")).toHaveTextContent("1");
    expect(screen.getByTestId("flow-wire-count-compute>output")).toHaveTextContent("1");
    fireEvent.click(screen.getByTestId("flow-wire-step-0>compute"));
    expect(onOpen).toHaveBeenCalledWith("compute");
  });

  it("taps the wire into the selected node, lists what crossed it per run, and filters by node", async () => {
    const user = userEvent.setup();
    const log: DebugEntry[] = [
      { message: 0, trace: BROKEN },
      { message: null, trace: GREEN },
    ];
    show(<Graph trace={GREEN} log={log} />);
    const sidebar = screen.getByTestId("flow-debug");
    expect(within(sidebar).getByText(debug.noTaps)).toBeInTheDocument();
    expect(screen.getByTestId("palette-debug")).toHaveAttribute("aria-disabled", "true");

    await user.click(screen.getByTestId("flow-node-compute"));
    await user.click(screen.getByTestId("palette-debug"));
    expect(screen.getByTestId("flow-tap-step-0>compute")).toBeInTheDocument();

    const rows = within(within(sidebar).getByRole("list", { name: debug.messages })).getAllByRole("listitem");
    // Newest first: the whole-sample run, then message 1, which carried the broken value.
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent(debug.wholeSample);
    expect(rows[1]).toHaveTextContent("Message 1");
    // T-5516: what crossed the wire reads as rows, never as JSON text.
    expect(within(rows[1]).getByText("many")).toBeInTheDocument();
    expect(rows[1].textContent).not.toContain("{\n");

    await user.selectOptions(within(sidebar).getByLabelText(debug.filter), "output");
    expect(within(sidebar).getByText(debug.nothingYet)).toBeInTheDocument();
    await user.selectOptions(within(sidebar).getByLabelText(debug.filter), "compute");
    expect(within(sidebar).getAllByRole("listitem")).toHaveLength(2);

    // The tap is a control of its own: pressing it removes the Debug node.
    fireEvent.keyDown(screen.getByTestId("flow-tap-step-0>compute"), { key: "Delete" });
    expect(screen.queryByTestId("flow-tap-step-0>compute")).toBeNull();
  });

  it("shows a failing node's error and the input that broke it, and reruns it alone on that input", async () => {
    const user = userEvent.setup();
    testPipeline.mockResolvedValue({ trace: { ...GREEN, errors: [{ stage: "mapping", message: "still a string" }] } });
    const toManifest = vi.fn((form: PipelineForm) => form);
    show(<NodeDebug project="helsinki" form={FORM} trace={BROKEN} nodes={toFlow(FORM).nodes} id="compute" toManifest={toManifest} />);

    expect(screen.getByText("expected number, got string")).toBeInTheDocument();
    expect(within(screen.getByTestId("flow-breaking-input")).getByText("many")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: debug.rerun }));
    // Only the compute step, on the pinned input as one JSON message.
    expect(toManifest).toHaveBeenCalledWith(expect.objectContaining({ processors: [], compute: FORM.compute }));
    expect(testPipeline).toHaveBeenCalledWith("helsinki", expect.anything(), {
      text: JSON.stringify([{ id: "s1", pm10: "many" }]),
      format: "json",
    });
    expect(await screen.findByText(debug.rerunFailed)).toBeInTheDocument();
    expect(screen.getByText("still a string")).toBeInTheDocument();
  });

  it("says what a node changed in the message", () => {
    show(<NodeDebug project="helsinki" form={FORM} trace={GREEN} nodes={toFlow(FORM).nodes} id="compute" toManifest={(form) => form} />);
    const changed = screen.getByRole("list", { name: debug.changed });
    expect(changed).toHaveTextContent('added type: "AirQualityObserved"');
  });

  it("runs one message of the sample at a time in step mode", async () => {
    const user = userEvent.setup();
    testPipeline.mockResolvedValue({ trace: GREEN });
    const entries: DebugEntry[] = [];
    show(
      <PipelineTest
        project="helsinki"
        draft={FORM}
        onChange={() => undefined}
        toManifest={(form) => form}
        onDebug={(entry) => entries.push(entry)}
      />,
    );
    const file = new File([JSON.stringify([{ id: "s1" }, { id: "s2" }])], "sample.json", { type: "application/json" });
    await user.upload(screen.getByLabelText(en.pipelines.test.chooseFile), file);
    await user.click(screen.getByLabelText(debug.stepMode));
    expect(screen.getByText("Message 1 of 2")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Run message 1 of 2" }));
    expect(testPipeline).toHaveBeenLastCalledWith("helsinki", FORM, { text: '[{"id":"s1"}]', format: "json" });
    await user.click(await screen.findByRole("button", { name: "Run message 2 of 2" }));
    expect(testPipeline).toHaveBeenLastCalledWith("helsinki", FORM, { text: '[{"id":"s2"}]', format: "json" });
    expect(entries.map((entry) => entry.message)).toEqual([0, 1]);

    await user.click(screen.getByRole("button", { name: debug.startOver }));
    expect(screen.getByText("Message 1 of 2")).toBeInTheDocument();
  });
});
