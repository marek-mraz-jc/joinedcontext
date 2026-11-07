import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import type { ReactNode } from "react";
import { AnalyserContext, parseAnswer, useAnalyser, useAnalysis, workerAnalyser } from "./analysis";
import type { AnalysisInput, AnalysisOutput } from "./analysis";

const OUTPUT = { source: "derived", vehicles: 0, readings: 0, first: null, last: null, stops: [], rides: 0, routes: [], cells: [], bands: [] } as unknown as AnalysisOutput;
const INPUT = { vehicles: [], origin: { lon: 24.94, lat: 60.17 }, bands: [10], wait: 5 } as unknown as AnalysisInput;

/** A worker of the page: what it was sent, and the means to answer as the real one does. */
class FakeWorker {
  static made: FakeWorker[] = [];
  sent: Array<{ id: number; input: string }> = [];
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: ErrorEvent) => void) | null = null;
  terminated = false;
  constructor() {
    FakeWorker.made.push(this);
  }
  postMessage(message: { id: number; input: string }) {
    this.sent.push(message);
  }
  terminate() {
    this.terminated = true;
  }
  answer(data: unknown) {
    this.onmessage?.({ data } as MessageEvent);
  }
}

afterEach(() => {
  FakeWorker.made.length = 0;
  vi.unstubAllGlobals();
});

describe("the analyser", () => {
  it("reads an answer, and says the module's own refusal", () => {
    expect(parseAnswer(JSON.stringify(OUTPUT))).toEqual(OUTPUT);
    expect(() => parseAnswer(JSON.stringify({ error: "no vehicles" }))).toThrow("no vehicles");
  });

  it("answers each call by its id from one worker, and starts a new one after it died", async () => {
    vi.stubGlobal("Worker", FakeWorker);
    const analyse = workerAnalyser();
    const first = analyse(INPUT);
    const second = analyse(INPUT);
    const worker = FakeWorker.made[0];
    expect(FakeWorker.made).toHaveLength(1);
    expect(worker.sent.map((m) => m.id)).toEqual([0, 1]);
    worker.answer({ id: 1, answer: JSON.stringify(OUTPUT) });
    worker.answer({ id: 99, answer: "{}" });
    worker.answer({ id: 0, error: "refused" });
    await expect(second).resolves.toEqual(OUTPUT);
    await expect(first).rejects.toThrow("refused");
    const broken = analyse(INPUT);
    worker.answer({ id: 2, answer: "not json" });
    await expect(broken).rejects.toThrow();
    const bad = analyse(INPUT);
    worker.answer({ id: 3, answer: JSON.stringify({ error: "bad point" }) });
    await expect(bad).rejects.toThrow("bad point");
    const lost = analyse(INPUT);
    const unsaid = analyse(INPUT);
    worker.onerror?.({ message: "out of memory" } as ErrorEvent);
    await expect(lost).rejects.toThrow("out of memory");
    await expect(unsaid).rejects.toThrow("out of memory");
    expect(worker.terminated).toBe(true);
    const again = analyse(INPUT);
    expect(FakeWorker.made).toHaveLength(2);
    FakeWorker.made[1].onerror?.({ message: "" } as ErrorEvent);
    await expect(again).rejects.toThrow("the analysis stopped");
  });

  it("uses the page's one worker when no test provides an analyser", () => {
    vi.stubGlobal("Worker", FakeWorker);
    const { result } = renderHook(() => useAnalyser());
    const { result: again } = renderHook(() => useAnalyser());
    expect(result.current).toBe(again.current);
  });

  it("runs again for a new input, drops a late answer, and says a failure", async () => {
    const calls: Array<{ resolve: (o: AnalysisOutput) => void; reject: (e: unknown) => void }> = [];
    const analyse = vi.fn(() => new Promise<AnalysisOutput>((resolve, reject) => calls.push({ resolve, reject })));
    const wrapper = ({ children }: { children: ReactNode }) => createElement(AnalyserContext.Provider, { value: analyse }, children);
    const { result, rerender } = renderHook(({ input }: { input: AnalysisInput | null }) => useAnalysis(input), { wrapper, initialProps: { input: null as AnalysisInput | null } });
    expect(result.current).toEqual({ output: null, running: false, error: null });
    rerender({ input: INPUT });
    expect(result.current.running).toBe(true);
    rerender({ input: { ...INPUT, bands: [20] } as AnalysisInput });
    await act(async () => calls[0].resolve(OUTPUT));
    expect(result.current.output).toBeNull();
    await act(async () => calls[1].reject(new Error("module failed")));
    await waitFor(() => expect(result.current.error?.message).toBe("module failed"));
    rerender({ input: { ...INPUT, bands: [30] } as AnalysisInput });
    await act(async () => calls[2].reject("not an error"));
    await waitFor(() => expect(result.current.error?.message).toBe("not an error"));
    rerender({ input: { ...INPUT, bands: [40] } as AnalysisInput });
    await act(async () => calls[3].resolve(OUTPUT));
    await waitFor(() => expect(result.current.output).toEqual(OUTPUT));
  });
});
