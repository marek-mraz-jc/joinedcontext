/**
 * The page's side of the analysis: the worker's answers matched to their calls, a refusal and a
 * dead worker said in words, and an answer to an older input dropped.
 */
import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AnalyserContext, parseAnswer, useAnalysis, workerAnalyser } from "./analysis";
import type { AnalysisInput, AnalysisOutput } from "./analysis";

const OUT: AnalysisOutput = { total: 0, kept: 0, unlocated: 0, untimed: 0, hexes: [], places: [], hourOfWeek: [], busiest: null, subCategories: [], first: null, last: null };
const INPUT: AnalysisInput = { alerts: [], filter: {} };

/** A worker that answers as the test says, so the page's side of the messages is what is tested. */
class FakeWorker {
  static made: FakeWorker[] = [];
  static answer: (worker: FakeWorker, id: number) => void = () => undefined;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: ((event: { message: string }) => void) | null = null;
  terminated = false;
  constructor() {
    FakeWorker.made.push(this);
  }
  postMessage(message: { id: number }): void {
    queueMicrotask(() => FakeWorker.answer(this, message.id));
  }
  terminate(): void {
    this.terminated = true;
  }
}

afterEach(() => {
  vi.unstubAllGlobals();
  FakeWorker.made.length = 0;
});

describe("the worker analyser", () => {
  it("hands each answer to its own call and says what the module refused or garbled", async () => {
    vi.stubGlobal("Worker", FakeWorker);
    const analyse = workerAnalyser();
    FakeWorker.answer = (worker, id) => {
      worker.onmessage?.({ data: { id: id + 100, answer: "{}" } });
      worker.onmessage?.({ data: { id, answer: JSON.stringify(OUT) } });
    };
    await expect(analyse(INPUT)).resolves.toEqual(OUT);
    FakeWorker.answer = (worker, id) => worker.onmessage?.({ data: { id, error: "the input is not JSON" } });
    await expect(analyse(INPUT)).rejects.toThrow("the input is not JSON");
    FakeWorker.answer = (worker, id) => worker.onmessage?.({ data: { id, answer: JSON.stringify({ error: "no alerts field" }) } });
    await expect(analyse(INPUT)).rejects.toThrow("no alerts field");
    FakeWorker.answer = (worker, id) => worker.onmessage?.({ data: { id } });
    await expect(analyse(INPUT)).rejects.toThrow(SyntaxError);
    expect(FakeWorker.made).toHaveLength(1);
  });

  it("tells every waiting call when the worker dies, and starts a new one for the next", async () => {
    vi.stubGlobal("Worker", FakeWorker);
    const analyse = workerAnalyser();
    FakeWorker.answer = (worker) => worker.onerror?.({ message: "" });
    await expect(analyse(INPUT)).rejects.toThrow("the analysis stopped");
    expect(FakeWorker.made[0].terminated).toBe(true);
    FakeWorker.answer = (worker) => worker.onerror?.({ message: "out of memory" });
    await expect(analyse(INPUT)).rejects.toThrow("out of memory");
    expect(FakeWorker.made).toHaveLength(2);
  });

  it("reads the module's answer, or its refusal", () => {
    expect(parseAnswer(JSON.stringify(OUT))).toEqual(OUT);
    expect(() => parseAnswer(JSON.stringify({ error: "bad filter" }))).toThrow("bad filter");
  });
});

function Probe({ input }: { input: AnalysisInput | null }) {
  const { output, running, error } = useAnalysis(input);
  return <p>{error ? `error: ${error.message}` : running ? "running" : output ? `kept ${output.kept}` : "idle"}</p>;
}

describe("useAnalysis", () => {
  it("runs nothing without input, drops a late answer to an older input, and says a refusal that is not an Error", async () => {
    const answers: Array<(out: AnalysisOutput) => void> = [];
    const analyser = vi.fn(() => new Promise<AnalysisOutput>((resolve) => answers.push(resolve)));
    const { rerender } = render(
      <AnalyserContext.Provider value={analyser}>
        <Probe input={null} />
      </AnalyserContext.Provider>,
    );
    expect(screen.getByText("idle")).toBeInTheDocument();
    rerender(
      <AnalyserContext.Provider value={analyser}>
        <Probe input={{ alerts: [], filter: { hour: 1 } }} />
      </AnalyserContext.Provider>,
    );
    rerender(
      <AnalyserContext.Provider value={analyser}>
        <Probe input={{ alerts: [], filter: { hour: 2 } }} />
      </AnalyserContext.Provider>,
    );
    await act(async () => answers[0]({ ...OUT, kept: 1 }));
    expect(screen.getByText("running")).toBeInTheDocument();
    await act(async () => answers[1]({ ...OUT, kept: 2 }));
    expect(screen.getByText("kept 2")).toBeInTheDocument();

    const refusing = () => Promise.reject("no worker");
    rerender(
      <AnalyserContext.Provider value={refusing}>
        <Probe input={{ alerts: [], filter: { hour: 3 } }} />
      </AnalyserContext.Provider>,
    );
    await waitFor(() => expect(screen.getByText("error: no worker")).toBeInTheDocument());
  });

  it("uses the page's one worker when no analyser is provided", async () => {
    vi.stubGlobal("Worker", FakeWorker);
    FakeWorker.answer = (worker, id) => worker.onmessage?.({ data: { id, answer: JSON.stringify({ ...OUT, kept: 5 }) } });
    render(<Probe input={INPUT} />);
    expect(await screen.findByText("kept 5")).toBeInTheDocument();
  });
});
