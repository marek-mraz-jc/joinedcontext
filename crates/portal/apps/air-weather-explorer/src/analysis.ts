import init, { analyse } from "../wasm/pkg/air_weather_explorer.js";

export interface Series {
  name: string;
  hourly: Array<number | null>;
  smooth: Array<number | null>;
  outliers: number[];
}

export interface Pair {
  air: string;
  weather: string;
  n: number;
  pearson: number | null;
  spearman: number | null;
}

export interface Analysis {
  hours: number[];
  air: Series[];
  weather: Series[];
  pairs: Pair[];
}

export interface AnalysisInput {
  air: Record<string, Array<[number, number]>>;
  weather: Record<string, Array<[number, number]>>;
  settings: { window?: number; threshold?: number };
}

/** What the module answers: the analysis, or the sentence of what it could not read. */
export function readAnswer(answer: string): Analysis {
  const parsed = JSON.parse(answer) as Analysis | { error: string };
  if ("error" in parsed) throw new Error(parsed.error);
  return parsed;
}

/** Runs in the worker, and in the page where there is no worker (a test, an old browser). */
export async function analyseHere(input: AnalysisInput): Promise<Analysis> {
  await init();
  return readAnswer(analyse(JSON.stringify(input)));
}

let worker: Worker | null = null;
let next = 0;
const waiting = new Map<number, { resolve: (result: Analysis) => void; reject: (error: Error) => void }>();

function theWorker(): Worker {
  if (worker) return worker;
  worker = new Worker(new URL("./analysis.worker.ts", import.meta.url), { type: "module" });
  worker.onmessage = (event: MessageEvent<{ id: number; result?: Analysis; error?: string }>) => {
    const entry = waiting.get(event.data.id);
    if (!entry) return;
    waiting.delete(event.data.id);
    if (event.data.result) entry.resolve(event.data.result);
    else entry.reject(new Error(event.data.error ?? "The analysis did not answer."));
  };
  worker.onerror = () => {
    for (const entry of waiting.values()) entry.reject(new Error("The analysis stopped. Reload the page."));
    waiting.clear();
    worker = null;
  };
  return worker;
}

/** The analysis, computed off the page's thread so the screen never stalls while it runs. */
export function computeAnalysis(input: AnalysisInput): Promise<Analysis> {
  if (typeof Worker === "undefined") return analyseHere(input);
  const id = (next += 1);
  return new Promise<Analysis>((resolve, reject) => {
    waiting.set(id, { resolve, reject });
    theWorker().postMessage({ id, input });
  });
}

/** The pair to show first: the strongest Spearman correlation over enough hours; `null` for none. */
export function strongest(pairs: Pair[]): Pair | null {
  let best: Pair | null = null;
  for (const pair of pairs) {
    if (pair.spearman === null) continue;
    if (!best || Math.abs(pair.spearman) > Math.abs(best.spearman ?? 0)) best = pair;
  }
  return best;
}

/** How strong a correlation is, in the page's words: under 0.2 it says nothing. */
export function strength(rho: number): "none" | "weak" | "moderate" | "strong" {
  const size = Math.abs(rho);
  return size < 0.2 ? "none" : size < 0.4 ? "weak" : size < 0.7 ? "moderate" : "strong";
}
