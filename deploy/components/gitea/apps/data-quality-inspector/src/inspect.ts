/**
 * Worker bridge and React hook for running WASM data quality inspection off the main thread.
 */
import { createContext, useContext, useEffect, useState } from "react";
import type { InspectInput, InspectOutput } from "./quality";

export type Inspector = (input: InspectInput) => Promise<InspectOutput>;

/** Parses the JSON output from the WebAssembly module, throwing if it returned an error. */
export function parseAnswer(text: string): InspectOutput {
  const answer = JSON.parse(text) as InspectOutput | { error: string };
  if ("error" in answer) throw new Error(answer.error);
  return answer;
}

/** Worker instance managing sequential message IDs. */
export function workerInspector(): Inspector {
  let worker: Worker | null = null;
  let next = 0;
  const waiting = new Map<number, { resolve: (out: InspectOutput) => void; reject: (error: Error) => void }>();

  const start = () => {
    worker = new Worker(new URL("./inspect.worker.ts", import.meta.url), { type: "module" });
    worker.onmessage = (event: MessageEvent<{ id: number; answer?: string; error?: string }>) => {
      const call = waiting.get(event.data.id);
      if (!call) return;
      waiting.delete(event.data.id);
      if (event.data.error !== undefined) {
        call.reject(new Error(event.data.error));
      } else {
        try {
          call.resolve(parseAnswer(event.data.answer ?? ""));
        } catch (error) {
          call.reject(error instanceof Error ? error : new Error(String(error)));
        }
      }
    };
    worker.onerror = (event) => {
      for (const call of waiting.values()) call.reject(new Error(event.message || "the inspector stopped"));
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

export const InspectorContext = createContext<Inspector | null>(null);

let shared: Inspector | null = null;

export function useInspector(): Inspector {
  const provided = useContext(InspectorContext);
  if (provided) return provided;
  shared ??= workerInspector();
  return shared;
}

/** Runs the inspection whenever input changes, discarding outdated responses. */
export function useInspect(input: InspectInput | null): {
  output: InspectOutput | null;
  running: boolean;
  error: Error | null;
} {
  const inspectFn = useInspector();
  const [state, setState] = useState<{ output: InspectOutput | null; running: boolean; error: Error | null }>({
    output: null,
    running: input !== null,
    error: null,
  });
  const key = input === null ? null : JSON.stringify(input);

  useEffect(() => {
    if (key === null) return;
    let current = true;
    setState((previous) => ({ ...previous, running: true, error: null }));
    inspectFn(JSON.parse(key) as InspectInput).then(
      (output) => current && setState({ output, running: false, error: null }),
      (error: unknown) =>
        current &&
        setState({
          output: null,
          running: false,
          error: error instanceof Error ? error : new Error(String(error)),
        }),
    );
    return () => {
      current = false;
    };
  }, [key, inspectFn]);

  return state;
}
