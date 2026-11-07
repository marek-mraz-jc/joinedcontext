/**
 * Comparison runner and React hook: offloads spatial aggregation to the Web Worker and
 * WebAssembly module, dropping stale answers from older inputs.
 */
import { createContext, useContext, useEffect, useState } from "react";
import type { CompareInput, CompareOutput } from "./districts";

export type Comparer = (input: CompareInput) => Promise<CompareOutput>;

/** Parses the JSON output from the WebAssembly module, throwing if it returned an error object. */
export function parseAnswer(text: string): CompareOutput {
  const answer = JSON.parse(text) as CompareOutput | { error: string };
  if ("error" in answer) throw new Error(answer.error);
  return answer;
}

/** Worker instance managing sequential message IDs. */
export function workerCompare(): Comparer {
  let worker: Worker | null = null;
  let next = 0;
  const waiting = new Map<number, { resolve: (out: CompareOutput) => void; reject: (error: Error) => void }>();
  const start = () => {
    worker = new Worker(new URL("./compare.worker.ts", import.meta.url), { type: "module" });
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
      for (const call of waiting.values()) call.reject(new Error(event.message || "the comparison stopped"));
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

export const ComparerContext = createContext<Comparer | null>(null);

let shared: Comparer | null = null;

export function useComparer(): Comparer {
  const provided = useContext(ComparerContext);
  if (provided) return provided;
  shared ??= workerCompare();
  return shared;
}

/** Runs the comparison whenever the input changes, discarding outdated responses. */
export function useCompare(input: CompareInput | null): { output: CompareOutput | null; running: boolean; error: Error | null } {
  const runCompare = useComparer();
  const [state, setState] = useState<{ output: CompareOutput | null; running: boolean; error: Error | null }>({
    output: null,
    running: input !== null,
    error: null,
  });
  const key = input === null ? null : JSON.stringify(input);

  useEffect(() => {
    if (key === null) return;
    let current = true;
    setState((previous) => ({ ...previous, running: true, error: null }));
    runCompare(JSON.parse(key) as CompareInput).then(
      (output) => current && setState({ output, running: false, error: null }),
      (error: unknown) =>
        current && setState({ output: null, running: false, error: error instanceof Error ? error : new Error(String(error)) }),
    );
    return () => {
      current = false;
    };
  }, [key, runCompare]);

  return state;
}
