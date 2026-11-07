import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import type { InspectInput, InspectOutput } from "./quality";

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

const INPUT: InspectInput = { now: 0, types: [] };
const OUTPUT: InspectOutput = { types: [] };

// The module keeps its one worker, so each case loads it afresh.
async function inspect() {
  vi.resetModules();
  return import("./inspect");
}

beforeEach(() => {
  FakeWorker.made = [];
  vi.stubGlobal("Worker", FakeWorker);
});
afterEach(() => vi.unstubAllGlobals());

describe("the inspection off the page's thread", () => {
  it("starts one worker, answers each call with its own answer and ignores a stray one", async () => {
    const { workerInspector } = await inspect();
    const run = workerInspector();
    const first = run(INPUT);
    const second = run(INPUT);
    expect(FakeWorker.made).toHaveLength(1);
    const [worker] = FakeWorker.made;
    const [a, b] = worker.posted.map((message) => message.id);
    expect(JSON.parse(worker.posted[0].input)).toEqual(INPUT);
    worker.answer({ id: 99, answer: JSON.stringify(OUTPUT) });
    worker.answer({ id: b, answer: JSON.stringify(OUTPUT) });
    worker.answer({ id: a, error: "no rows" });
    await expect(second).resolves.toEqual(OUTPUT);
    await expect(first).rejects.toThrow("no rows");
  });

  it("rejects an answer that is an error object or no JSON at all", async () => {
    const { workerInspector } = await inspect();
    const run = workerInspector();
    const refused = run(INPUT);
    const garbled = run(INPUT);
    const [worker] = FakeWorker.made;
    worker.answer({ id: worker.posted[0].id, answer: JSON.stringify({ error: "the input could not be read" }) });
    worker.answer({ id: worker.posted[1].id });
    await expect(refused).rejects.toThrow("the input could not be read");
    await expect(garbled).rejects.toThrow();
  });

  it("fails every waiting call when the worker stops, and starts a new one for the next", async () => {
    const { workerInspector } = await inspect();
    const run = workerInspector();
    const waiting = [run(INPUT), run(INPUT)];
    const [worker] = FakeWorker.made;
    worker.onerror?.({ message: "" });
    for (const one of waiting) await expect(one).rejects.toThrow("the inspector stopped");
    expect(worker.terminated).toBe(true);
    const next = run(INPUT);
    expect(FakeWorker.made).toHaveLength(2);
    // The worker's own words, where it has any.
    FakeWorker.made[1].onerror?.({ message: "out of memory" });
    await expect(next).rejects.toThrow("out of memory");
  });

  it("gives the page the shared worker inspector when none is provided, and words a failure", async () => {
    const { useInspect } = await inspect();
    function Probe() {
      const { output, running, error } = useInspect(INPUT);
      return <p>{error ? `failed: ${error.message}` : running ? "running" : output ? `types ${output.types.length}` : "none"}</p>;
    }
    const view = render(<Probe />);
    expect(screen.getByText("running")).toBeInTheDocument();
    const [worker] = FakeWorker.made;
    worker.answer({ id: worker.posted[0].id, error: "too little" });
    expect(await screen.findByText("failed: too little")).toBeInTheDocument();
    view.unmount();
  });

  it("drops an answer for an input that has since changed", async () => {
    const { InspectorContext, useInspect } = await inspect();
    const answers: Array<(output: InspectOutput) => void> = [];
    const inspector = (input: InspectInput) => new Promise<InspectOutput>((resolve) => answers.push(() => resolve({ types: Array.from({ length: input.now }, () => OUTPUT.types[0]) })));
    function Probe({ input }: { input: InspectInput | null }) {
      const { output } = useInspect(input);
      return <p>{output ? `types ${output.types.length}` : "none"}</p>;
    }
    const view = render(
      <InspectorContext.Provider value={inspector}>
        <Probe input={INPUT} />
      </InspectorContext.Provider>,
    );
    view.rerender(
      <InspectorContext.Provider value={inspector}>
        <Probe input={{ ...INPUT, now: 3 }} />
      </InspectorContext.Provider>,
    );
    answers[0](OUTPUT);
    answers[1](OUTPUT);
    expect(await screen.findByText("types 3")).toBeInTheDocument();
    view.rerender(
      <InspectorContext.Provider value={inspector}>
        <Probe input={null} />
      </InspectorContext.Provider>,
    );
    // No input (the types are read again): the last inspection stays on screen meanwhile.
    await waitFor(() => expect(screen.getByText("types 3")).toBeInTheDocument());
  });

  it("words a failure that is no Error too", async () => {
    const { InspectorContext, useInspect } = await inspect();
    function Probe() {
      const { error } = useInspect(INPUT);
      return <p>{error ? `failed: ${error.message}` : "none"}</p>;
    }
    render(
      <InspectorContext.Provider value={() => Promise.reject("bare")}>
        <Probe />
      </InspectorContext.Provider>,
    );
    expect(await screen.findByText("failed: bare")).toBeInTheDocument();
  });
});
