import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import type { CompareInput, CompareOutput } from "./districts";

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

const INPUT = { districts: [], events: [], bikes: [], alerts: [], air: [] } as unknown as CompareInput;
const OUTPUT: CompareOutput = { districts: [], outside: { events: 0, bikes: 0, alerts: 0, air: 0 } };

// The module keeps its one worker, so each case loads it afresh.
async function compare() {
  vi.resetModules();
  return import("./compare");
}

beforeEach(() => {
  FakeWorker.made = [];
  vi.stubGlobal("Worker", FakeWorker);
});
afterEach(() => vi.unstubAllGlobals());

describe("the comparison off the page's thread", () => {
  it("starts one worker, answers each call with its own answer and ignores a stray one", async () => {
    const { workerCompare } = await compare();
    const run = workerCompare();
    const first = run(INPUT);
    const second = run(INPUT);
    expect(FakeWorker.made).toHaveLength(1);
    const [worker] = FakeWorker.made;
    const [a, b] = worker.posted.map((message) => message.id);
    expect(JSON.parse(worker.posted[0].input)).toEqual(INPUT);
    worker.answer({ id: 99, answer: JSON.stringify(OUTPUT) });
    worker.answer({ id: b, answer: JSON.stringify(OUTPUT) });
    worker.answer({ id: a, error: "no districts" });
    await expect(second).resolves.toEqual(OUTPUT);
    await expect(first).rejects.toThrow("no districts");
  });

  it("rejects an answer that is an error object or no JSON at all", async () => {
    const { workerCompare } = await compare();
    const run = workerCompare();
    const refused = run(INPUT);
    const garbled = run(INPUT);
    const [worker] = FakeWorker.made;
    worker.answer({ id: worker.posted[0].id, answer: JSON.stringify({ error: "the input could not be read" }) });
    worker.answer({ id: worker.posted[1].id });
    await expect(refused).rejects.toThrow("the input could not be read");
    await expect(garbled).rejects.toThrow();
  });

  it("fails every waiting call when the worker stops, and starts a new one for the next", async () => {
    const { workerCompare } = await compare();
    const run = workerCompare();
    const waiting = [run(INPUT), run(INPUT)];
    const [worker] = FakeWorker.made;
    worker.onerror?.({ message: "" });
    for (const one of waiting) await expect(one).rejects.toThrow("the comparison stopped");
    expect(worker.terminated).toBe(true);
    const next = run(INPUT);
    expect(FakeWorker.made).toHaveLength(2);
    // The worker's own words, where it has any.
    FakeWorker.made[1].onerror?.({ message: "out of memory" });
    await expect(next).rejects.toThrow("out of memory");
  });

  it("gives the page the shared worker comparison when none is provided, and words a failure", async () => {
    const { useCompare } = await compare();
    function Probe() {
      const { output, running, error } = useCompare(INPUT);
      return <p>{error ? `failed: ${error.message}` : running ? "running" : output ? `districts ${output.districts.length}` : "none"}</p>;
    }
    const view = render(<Probe />);
    expect(screen.getByText("running")).toBeInTheDocument();
    const [worker] = FakeWorker.made;
    worker.answer({ id: worker.posted[0].id, error: "too little" });
    expect(await screen.findByText("failed: too little")).toBeInTheDocument();
    view.unmount();
  });

  it("drops an answer for an input that has since changed", async () => {
    const { ComparerContext, useCompare } = await compare();
    const answers: Array<(output: CompareOutput) => void> = [];
    const comparer = (input: CompareInput) => new Promise<CompareOutput>((resolve) => answers.push(() => resolve({ ...OUTPUT, outside: { ...OUTPUT.outside, events: input.events.length } })));
    function Probe({ input }: { input: CompareInput | null }) {
      const { output } = useCompare(input);
      return <p>{output ? `outside ${output.outside.events}` : "none"}</p>;
    }
    const view = render(
      <ComparerContext.Provider value={comparer}>
        <Probe input={INPUT} />
      </ComparerContext.Provider>,
    );
    view.rerender(
      <ComparerContext.Provider value={comparer}>
        <Probe input={{ ...INPUT, events: [1, 2, 3] } as unknown as CompareInput} />
      </ComparerContext.Provider>,
    );
    answers[0](OUTPUT);
    answers[1](OUTPUT);
    expect(await screen.findByText("outside 3")).toBeInTheDocument();
    view.rerender(
      <ComparerContext.Provider value={comparer}>
        <Probe input={null} />
      </ComparerContext.Provider>,
    );
    // No input (the rows are read again): the last comparison stays on screen meanwhile.
    await waitFor(() => expect(screen.getByText("outside 3")).toBeInTheDocument());
  });

  it("words a failure that is no Error too", async () => {
    const { ComparerContext, useCompare } = await compare();
    function Probe() {
      const { error } = useCompare(INPUT);
      return <p>{error ? `failed: ${error.message}` : "none"}</p>;
    }
    render(
      <ComparerContext.Provider value={() => Promise.reject("bare")}>
        <Probe />
      </ComparerContext.Provider>,
    );
    expect(await screen.findByText("failed: bare")).toBeInTheDocument();
  });
});
