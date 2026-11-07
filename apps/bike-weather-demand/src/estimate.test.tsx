import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import type { EstimateInput, EstimateOutput } from "./bikes";

/** A stand-in for the browser's Worker: it keeps what the page posts and answers when told to. */
class FakeWorker {
  static made: FakeWorker[] = [];
  onmessage: ((event: { data: { id: number; answer?: string; error?: string } }) => void) | null = null;
  onerror: ((event: { message: string }) => void) | null = null;
  posted: Array<{ id: number; input: string }> = [];
  terminated = false;
  constructor() {
    FakeWorker.made.push(this);
  }
  postMessage(message: { id: number; input: string }) {
    this.posted.push(message);
  }
  terminate() {
    this.terminated = true;
  }
  answer(data: { id: number; answer?: string; error?: string }) {
    this.onmessage?.({ data });
  }
}

const INPUT = { now: 0, totalSlots: 10, bikes: [], weather: [] } as unknown as EstimateInput;
const OUTPUT: EstimateOutput = { hours: 0, profile: [], weather: null, sigma: null, estimate: [], assumption: null, enough: false };

// The module keeps its one worker, so each case loads it afresh.
async function estimate() {
  vi.resetModules();
  return import("./estimate");
}

beforeEach(() => {
  FakeWorker.made = [];
  vi.stubGlobal("Worker", FakeWorker);
});
afterEach(() => vi.unstubAllGlobals());

describe("the estimate off the page's thread", () => {
  it("starts one worker, answers each call with its own answer and ignores a stray one", async () => {
    const { workerEstimate } = await estimate();
    const run = workerEstimate();
    const first = run(INPUT);
    const second = run(INPUT);
    expect(FakeWorker.made).toHaveLength(1);
    const [worker] = FakeWorker.made;
    const [a, b] = worker.posted.map((message) => message.id);
    expect(JSON.parse(worker.posted[0].input)).toEqual(INPUT);
    worker.answer({ id: 99, answer: JSON.stringify(OUTPUT) });
    worker.answer({ id: b, answer: JSON.stringify(OUTPUT) });
    worker.answer({ id: a, error: "no history" });
    await expect(second).resolves.toEqual(OUTPUT);
    await expect(first).rejects.toThrow("no history");
  });

  it("rejects an answer that is an error object or no JSON at all", async () => {
    const { workerEstimate } = await estimate();
    const run = workerEstimate();
    const refused = run(INPUT);
    const garbled = run(INPUT);
    const [worker] = FakeWorker.made;
    worker.answer({ id: worker.posted[0].id, answer: JSON.stringify({ error: "the estimate could not be read" }) });
    worker.answer({ id: worker.posted[1].id });
    await expect(refused).rejects.toThrow("the estimate could not be read");
    await expect(garbled).rejects.toThrow();
  });

  it("fails every waiting call when the worker stops, and starts a new one for the next", async () => {
    const { workerEstimate } = await estimate();
    const run = workerEstimate();
    const waiting = [run(INPUT), run(INPUT)];
    const [worker] = FakeWorker.made;
    worker.onerror?.({ message: "" });
    for (const one of waiting) await expect(one).rejects.toThrow("the estimation stopped");
    expect(worker.terminated).toBe(true);
    const next = run(INPUT);
    expect(FakeWorker.made).toHaveLength(2);
    // The worker's own words, where it has any.
    FakeWorker.made[1].onerror?.({ message: "out of memory" });
    await expect(next).rejects.toThrow("out of memory");
  });

  it("gives the page the shared worker estimate when no estimater is provided, and words a failure", async () => {
    const { useEstimate } = await estimate();
    function Probe() {
      const { output, running, error } = useEstimate(INPUT);
      return <p>{error ? `failed: ${error.message}` : running ? "running" : output ? `hours ${output.hours}` : "none"}</p>;
    }
    const view = render(<Probe />);
    expect(screen.getByText("running")).toBeInTheDocument();
    const [worker] = FakeWorker.made;
    worker.answer({ id: worker.posted[0].id, error: "too little" });
    expect(await screen.findByText("failed: too little")).toBeInTheDocument();
    view.unmount();
  });

  it("drops an answer for an input that has since changed", async () => {
    const { EstimaterContext, useEstimate } = await estimate();
    const answers: Array<(output: EstimateOutput) => void> = [];
    const estimater = (input: EstimateInput) => new Promise<EstimateOutput>((resolve) => answers.push(() => resolve({ ...OUTPUT, hours: input.totalSlots })));
    function Probe({ input }: { input: EstimateInput | null }) {
      const { output } = useEstimate(input);
      return <p>{output ? `hours ${output.hours}` : "none"}</p>;
    }
    const view = render(
      <EstimaterContext.Provider value={estimater}>
        <Probe input={INPUT} />
      </EstimaterContext.Provider>,
    );
    view.rerender(
      <EstimaterContext.Provider value={estimater}>
        <Probe input={{ ...INPUT, totalSlots: 30 }} />
      </EstimaterContext.Provider>,
    );
    answers[0](OUTPUT);
    answers[1](OUTPUT);
    expect(await screen.findByText("hours 30")).toBeInTheDocument();
    view.rerender(
      <EstimaterContext.Provider value={estimater}>
        <Probe input={null} />
      </EstimaterContext.Provider>,
    );
    await waitFor(() => expect(screen.getByText("none")).toBeInTheDocument());
  });
});
