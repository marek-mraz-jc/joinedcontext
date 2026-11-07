/**
 * The flow painted from the running stream (T-3090, PL-66): each node shows the counters of its
 * own label, the step a message stops at is the one shown failing, and a test result on screen
 * takes the place of the live numbers.
 */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import type { NodeCounters } from "../src/pages/pipelines/PipelineFlow";
import { PipelineFlow, QUEUE_WARN, paintOfCounters, queueOf, toFlow } from "../src/pages/pipelines/PipelineFlow";
import type { Trace } from "../src/pages/pipelines/PipelineTest";

const form: PipelineForm = {
  name: "bikes",
  class: "auto",
  source: { dataSourceRef: "feed-bikes" },
  moreSources: [{ dataSourceRef: "feed-docks" }],
  processors: [{ step: { processor: { dedupe: { cache: "c", key: "${! this.id }" } } } }],
  compute: { kind: "bloblang", bloblang: "root = this" },
  targetEndpoint: "urn:ngsi-ld:Endpoint:hel.fi:mobility:ep-bikes",
} as PipelineForm;

const counters: NodeCounters = {
  input: { received: 1200, sent: 1200 },
  step_0: { received: 1200, sent: 1180, errors: 20 },
  compute: { received: 1180, sent: 1180 },
  output: { received: 1180, sent: 1180, errors: null },
  processor_7: { received: 1180, sent: 1180 },
};

const sentence = (count: number) => `${count} errors`;

describe("paintOfCounters", () => {
  it("gives every node the counters of its own label", () => {
    const { nodes } = toFlow(form);
    const paint = paintOfCounters(counters, nodes, sentence);
    // Both sources are the one `input` the runner merges them into.
    expect(paint.source).toEqual({ eventsIn: 1200, eventsOut: 1200, state: "ok", error: undefined });
    expect(paint["source-0"]).toEqual(paint.source);
    expect(paint["step-0"]).toEqual({ eventsIn: 1200, eventsOut: 1180, state: "error", error: "20 errors" });
    expect(paint.compute.state).toBe("ok");
    expect(paint.output).toEqual({ eventsIn: 1180, eventsOut: 1180, state: "ok", error: undefined });
  });

  it("leaves a node the runner says nothing about idle", () => {
    const { nodes } = toFlow(form);
    const paint = paintOfCounters({ input: { received: 3, sent: 3 } }, nodes, sentence);
    expect(paint.compute).toEqual({ state: "idle" });
    expect(paint.output).toEqual({ state: "idle" });
  });
});

function renderFlow(props: { trace?: Trace | null; live?: NodeCounters; bufferDepth?: number | null }) {
  render(
    <QueryClientProvider client={new QueryClient()}>
      <I18nextProvider i18n={i18n}>
        <PipelineFlow
          form={form}
          onChange={() => undefined}
          trace={props.trace ?? null}
          live={props.live}
          bufferDepth={props.bufferDepth}
          selected={null}
          onSelect={() => undefined}
          dataSources={[]}
          endpoints={[]}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the live flow", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("shows the failing step of the running stream and says the numbers are live", () => {
    renderFlow({ live: counters });
    expect(screen.getByTestId("flow-live")).toHaveTextContent(en.pipelines.flow.live);
    expect(screen.getByTestId("flow-node-step-0")).toHaveAttribute("data-state", "error");
    expect(screen.getByTestId("flow-node-step-0")).toHaveTextContent("20 errors in the last 5 minutes");
    expect(screen.getByTestId("flow-node-compute")).toHaveAttribute("data-state", "ok");
  });

  it("gives way to a test result, which is what the author just asked for", () => {
    const trace = { input: { events: 1 }, mapping: [{}], validation: [{ ok: true }], errors: [] } as unknown as Trace;
    renderFlow({ trace, live: counters });
    expect(screen.queryByTestId("flow-live")).toBeNull();
    expect(screen.getByTestId("flow-node-step-0")).toHaveAttribute("data-state", "ok");
  });

  it("paints nothing live for a stream that reports no node", () => {
    renderFlow({ live: {} });
    expect(screen.queryByTestId("flow-live")).toBeNull();
    expect(screen.getByTestId("flow-node-step-0")).toHaveAttribute("data-state", "idle");
  });
});

describe("the queue before the output (T-3973)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("reads no depth as nothing, zero as an empty queue, and past the limit as high", () => {
    expect(queueOf(undefined)).toBeUndefined();
    expect(queueOf(null)).toBeUndefined();
    expect(queueOf(-1)).toBeUndefined();
    expect(queueOf(0)).toEqual({ depth: 0, high: false });
    expect(queueOf(QUEUE_WARN)).toEqual({ depth: QUEUE_WARN, high: false });
    expect(queueOf(QUEUE_WARN + 1)).toEqual({ depth: QUEUE_WARN + 1, high: true });
  });

  it("draws nothing on the wire while the runner reports no depth", () => {
    renderFlow({ live: counters });
    expect(screen.queryByTestId("flow-queue")).toBeNull();
  });

  it("draws an empty queue quietly, with the sentence for a screen reader", () => {
    renderFlow({ live: counters, bufferDepth: 0 });
    const queue = screen.getByTestId("flow-queue");
    expect(queue).toHaveTextContent("0 queued");
    expect(queue).not.toHaveAttribute("data-high");
    expect(queue.querySelector("title")?.textContent).toBe("0 messages wait in the buffer before the output");
  });

  it("marks a queue past the limit and says the output is not keeping up", () => {
    renderFlow({ bufferDepth: 250 });
    const queue = screen.getByTestId("flow-queue");
    expect(queue).toHaveAttribute("data-high", "true");
    expect(queue).toHaveClass("fill-warning");
    expect(queue.querySelector("title")?.textContent).toContain("more than 100: the output is not keeping up");
  });
});
