/**
 * Demand and weather estimation runner and React hook: offloads time series aggregation and
 * residual regression to the Web Worker and WebAssembly module, discarding stale responses.
 */
import { createContext, useContext, useEffect, useState } from "react";
import type { EstimateInput, EstimateOutput } from "./bikes";

export type Estimater = (input: EstimateInput) => Promise<EstimateOutput>;

/** Parses the JSON output from the WebAssembly module, throwing if it returned an error object. */
export function parseAnswer(text: string): EstimateOutput {
  const answer = JSON.parse(text) as EstimateOutput | { error: string };
  if ("error" in answer) throw new Error(answer.error);
  return answer;
}

/** Worker instance managing sequential message IDs. */
export function workerEstimate(): Estimater {
  let worker: Worker | null = null;
  let next = 0;
  const waiting = new Map<number, { resolve: (out: EstimateOutput) => void; reject: (error: Error) => void }>();
  const start = () => {
    worker = new Worker(new URL("./estimate.worker.ts", import.meta.url), { type: "module" });
    worker.onmessage = (event: MessageEvent<{ id: number; answer?: string; error?: string }>) => {
      const call = waiting.get(event.data.id);
      if (!call) return;
      waiting.delete(event.data.id);
      if (event.data.error !== undefined) call.reject(new Error(event.data.error));
      else {
        try {
          call.resolve(parseAnswer(event.data.answer ?? ""));
        } catch (error) {
          call.reject(error instanceof Error ? error : new Error(String(error)));
        }
      }
    };
    worker.onerror = (event) => {
      for (const call of waiting.values()) call.reject(new Error(event.message || "the estimation stopped"));
      waiting.clear();
      worker?.terminate();
      worker = null;
    };
  };
  return (input) =>
    new Promise((resolve, reject) => {
      if (!worker) start();
      const id = next++;
      waiting.set(id, { resolve, reject });
      worker!.postMessage({ id, input: JSON.stringify(input) });
    });
}

export const EstimaterContext = createContext<Estimater | null>(null);

let shared: Estimater | null = null;

export function useEstimater(): Estimater {
  const provided = useContext(EstimaterContext);
  if (provided) return provided;
  shared ??= workerEstimate();
  return shared;
}

/** Runs the estimation whenever the input changes, discarding outdated responses. */
export function useEstimate(
  input: EstimateInput | null,
): { output: EstimateOutput | null; running: boolean; error: Error | null } {
  const runEstimate = useEstimater();
  const [state, setState] = useState<{ output: EstimateOutput | null; running: boolean; error: Error | null }>({
    output: null,
    running: input !== null,
    error: null,
  });
  const key = input === null ? null : JSON.stringify(input);

  useEffect(() => {
    if (key === null) {
      setState({ output: null, running: false, error: null });
      return;
    }
    let current = true;
    setState((previous) => ({ ...previous, running: true, error: null }));
    runEstimate(JSON.parse(key) as EstimateInput).then(
      (output) => current && setState({ output, running: false, error: null }),
      (error: unknown) =>
        current && setState({ output: null, running: false, error: error instanceof Error ? error : new Error(String(error)) }),
    );
    return () => {
      current = false;
    };
  }, [key, runEstimate]);

  return state;
}
