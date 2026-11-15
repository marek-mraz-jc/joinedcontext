import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AnalyserContext, parseAnswer, useAnalyser, useAnalysis, workerAnalyser } from "./analysis";
import type { AnalysisInput, AnalysisOutput, Analyser } from "./analysis";

/** A Worker that answers when the test says, so order, failure and death can be played out. */
class FakeWorker {
  static made: FakeWorker[] = [];
  posted: { id: number; input: string }[] = [];
  onmessage: ((event: MessageEvent<{ id: number; answer?: string; error?: string }>) => void) | null = null;
  onerror: ((event: { message: string }) => void) | null = null;
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
  answer(id: number, data: { answer?: string; error?: string }) {
    this.onmessage?.({ data: { id, ...data } } as MessageEvent<{ id: number; answer?: string; error?: string }>);
  }
}

const OUT = { results: [], rising: 0, falling: 0, flat: 0, short: 0, anomalies: 0 } as AnalysisOutput;
const INPUT: AnalysisInput = { series: [] };

describe("the worker analyser", () => {
  beforeEach(() => {
    FakeWorker.made = [];
    vi.stubGlobal("Worker", FakeWorker);
  });
  afterEach(() => vi.unstubAllGlobals());

  it("starts one worker and answers each call by its own id, in whatever order", async () => {
    const analyse = workerAnalyser();
    const first = analyse(INPUT);
    const second = analyse({ series: [{ id: "a", points: [] }] });
    const worker = FakeWorker.made[0];
    expect(FakeWorker.made).toHaveLength(1);
    expect(worker.posted.map((p) => p.id)).toEqual([0, 1]);
    worker.answer(1, { answer: JSON.stringify({ ...OUT, short: 1 }) });
    worker.answer(0, { answer: JSON.stringify(OUT) });
    worker.answer(7, { answer: JSON.stringify(OUT) });
    await expect(second).resolves.toMatchObject({ short: 1 });
    await expect(first).resolves.toEqual(OUT);
  });

  it("rejects a call the module refused, or answered with what is no answer", async () => {
    const analyse = workerAnalyser();
    const refused = analyse(INPUT);
    const garbled = analyse(INPUT);
    const failed = analyse(INPUT);
    const worker = FakeWorker.made[0];
    worker.answer(0, { answer: JSON.stringify({ error: "the series could not be read" }) });
    worker.answer(1, { answer: "not json" });
    worker.answer(2, { error: "the module failed to load" });
    await expect(refused).rejects.toThrow("the series could not be read");
    await expect(garbled).rejects.toThrow();
    await expect(failed).rejects.toThrow("the module failed to load");
  });

  it("tells every waiting call when the worker dies, and starts a new one for the next", async () => {
    const analyse = workerAnalyser();
    const a = analyse(INPUT);
    const b = analyse(INPUT);
    const dead = FakeWorker.made[0];
    dead.onerror?.({ message: "" });
    await expect(a).rejects.toThrow("the analysis stopped");
    await expect(b).rejects.toThrow("the analysis stopped");
    expect(dead.terminated).toBe(true);
    const c = analyse(INPUT);
    expect(FakeWorker.made).toHaveLength(2);
    FakeWorker.made[1].onerror?.({ message: "out of memory" });
    await expect(c).rejects.toThrow("out of memory");
  });

  it("reads the module's answer, and throws its error", () => {
    expect(parseAnswer(JSON.stringify(OUT))).toEqual(OUT);
    expect(() => parseAnswer(JSON.stringify({ error: "no" }))).toThrow("no");
  });

  it("is the page's analyser when no test provides one, one worker for the page", () => {
    const { result, rerender } = renderHook(() => useAnalyser());
    const first = result.current;
    rerender();
    expect(result.current).toBe(first);
  });
});

describe("the analysis of a changing input", () => {
  function withAnalyser(analyse: Analyser) {
    return ({ children }: { children: ReactNode }) => <AnalyserContext.Provider value={analyse}>{children}</AnalyserContext.Provider>;
  }

  it("runs nothing for no input", () => {
    const analyse = vi.fn<Analyser>();
    const { result } = renderHook(() => useAnalysis(null), { wrapper: withAnalyser(analyse) });
    expect(result.current).toEqual({ output: null, running: false, error: null });
    expect(analyse).not.toHaveBeenCalled();
  });

  it("keeps the newest answer and drops one that arrives late for an older input", async () => {
    const pending: { input: AnalysisInput; resolve: (out: AnalysisOutput) => void }[] = [];
    const analyse: Analyser = (input) => new Promise((resolve) => pending.push({ input, resolve }));
    const { result, rerender } = renderHook(({ input }) => useAnalysis(input), { wrapper: withAnalyser(analyse), initialProps: { input: INPUT } });
    expect(result.current.running).toBe(true);
    rerender({ input: { series: [{ id: "b", points: [] }] } });
    await waitFor(() => expect(pending).toHaveLength(2));
    await act(async () => pending[1].resolve({ ...OUT, flat: 2 }));
    await act(async () => pending[0].resolve({ ...OUT, flat: 9 }));
    expect(result.current).toEqual({ output: { ...OUT, flat: 2 }, running: false, error: null });
  });

  it("says why an analysis failed, a thrown value made an Error", async () => {
    const { result } = renderHook(() => useAnalysis(INPUT), { wrapper: withAnalyser(() => Promise.reject("the worker is gone")) });
    await waitFor(() => expect(result.current.error?.message).toBe("the worker is gone"));
    expect(result.current.running).toBe(false);
  });
});
