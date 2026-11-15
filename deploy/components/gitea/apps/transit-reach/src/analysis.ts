/**
 * The analysis as the page sees it: what it hands the Rust module and what comes back, and the
 * analyser that runs it in a Web Worker so the page never waits on it (AP-142). A test hands the
 * page an analyser that calls the same module in-process (`AnalyserContext`).
 */
import { createContext, useContext, useEffect, useState } from "react";

export interface VehicleInput {
  id: string;
  /** `t` in milliseconds since the epoch; `speed` in metres a second. */
  points: { t: number; lon: number; lat: number; speed?: number; route?: string }[];
}

/** HSL's stops and lines (T-3356): a line names its stops by `id`. */
export interface NetworkInput {
  stops: { id: string; lon: number; lat: number; name?: string; code?: string }[];
  routes: { name: string; mode?: string; stops: string[] }[];
}

export interface AnalysisInput {
  vehicles: VehicleInput[];
  /** Used over the vehicles when it holds a stop and a line. */
  network?: NetworkInput;
  origin: { lon: number; lat: number };
  /** The minutes each band ends at; default 10, 20 and 30. */
  bands?: number[];
  /** Minutes waited at each boarding; default 5. */
  wait?: number;
  hexSize?: number;
}

export interface StopOut {
  lon: number;
  lat: number;
  /** The name and sign code from HSL's register; `null` for a stop derived from the vehicles. */
  name: string | null;
  code: string | null;
  vehicles: number;
  routes: string[];
  /** When it is reached on foot from the point; `null` beyond the last band. */
  minutes: number | null;
}

export interface Cell {
  ring: [number, number][];
  minutes: number;
  band: number;
}

export interface Band {
  minutes: number;
  areaKm2: number;
  stops: number;
}

export interface AnalysisOutput {
  origin: { lon: number; lat: number };
  /** What the stops and rides came from. */
  source: "network" | "vehicles";
  vehicles: number;
  readings: number;
  first: number | null;
  last: number | null;
  stops: StopOut[];
  rides: number;
  routes: string[];
  cells: Cell[];
  bands: Band[];
}

export type Analyser = (input: AnalysisInput) => Promise<AnalysisOutput>;

/** What the module answers: the output, or `{error}` for input it could not read. */
export function parseAnswer(text: string): AnalysisOutput {
  const answer = JSON.parse(text) as AnalysisOutput | { error: string };
  if ("error" in answer) throw new Error(answer.error);
  return answer;
}

/** The analyser of the browser: one worker for the page, each call answered by its own id. */
export function workerAnalyser(): Analyser {
  let worker: Worker | null = null;
  let next = 0;
  const waiting = new Map<number, { resolve: (out: AnalysisOutput) => void; reject: (error: Error) => void }>();
  const start = () => {
    worker = new Worker(new URL("./analysis.worker.ts", import.meta.url), { type: "module" });
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
      // A worker that died answers nobody: every waiting call is told, and the next one starts a new worker.
      for (const call of waiting.values()) call.reject(new Error(event.message || "the analysis stopped"));
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

export const AnalyserContext = createContext<Analyser | null>(null);

let shared: Analyser | null = null;

/** The page's analyser: the one a test provides, else the page's one worker. */
export function useAnalyser(): Analyser {
  const provided = useContext(AnalyserContext);
  if (provided) return provided;
  shared ??= workerAnalyser();
  return shared;
}

/**
 * The analysis of an input, run again whenever the input changes; the answer to an older input
 * that arrives late is dropped.
 */
export function useAnalysis(input: AnalysisInput | null): { output: AnalysisOutput | null; running: boolean; error: Error | null } {
  const analyse = useAnalyser();
  const [state, setState] = useState<{ output: AnalysisOutput | null; running: boolean; error: Error | null }>({
    output: null,
    running: input !== null,
    error: null,
  });
  const key = input === null ? null : JSON.stringify(input);
  // The input is compared by its JSON: a new object with the same vehicles and point is no new run.
  useEffect(() => {
    if (key === null) return;
    let current = true;
    setState((previous) => ({ ...previous, running: true, error: null }));
    analyse(JSON.parse(key) as AnalysisInput).then(
      (output) => current && setState({ output, running: false, error: null }),
      (error: unknown) => current && setState({ output: null, running: false, error: error instanceof Error ? error : new Error(String(error)) }),
    );
    return () => {
      current = false;
    };
  }, [key, analyse]);
  return state;
}
