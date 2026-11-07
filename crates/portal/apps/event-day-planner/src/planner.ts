import init, { plan } from "../wasm/pkg/event_day_planner.js";

/** An event as the day planner reads it (wasm/src/lib.rs `Event`): times in epoch milliseconds. */
export interface PlanEvent {
  id: string;
  name: string;
  address: string;
  start: number | null;
  end: number | null;
  lon: number | null;
  lat: number | null;
}

export interface Settings {
  walkKmh?: number;
  wholeUpToMinutes?: number;
  stayMinutes?: number;
  chosen?: string[];
  dayStart?: number | null;
  now?: number;
}

export type Fit = "ok" | "late" | "missed";

export interface Item {
  id: string;
  name: string;
  begin: number;
  finish: number;
  walkMinutes: number;
  walkKm: number;
  fit: Fit;
  lateMinutes: number;
  located: boolean;
}

export interface Day {
  items: Item[];
  conflicts: Array<[string, string]>;
  walkKm: number;
  walkMinutes: number;
  suggested: boolean;
  ics: string;
  unknown: string[];
}

export interface PlanInput {
  events: PlanEvent[];
  settings: Settings;
}

/** What the module answers: the day, or the sentence of what it could not read. */
export function readAnswer(answer: string): Day {
  const parsed = JSON.parse(answer) as Day | { error: string };
  if ("error" in parsed) throw new Error(parsed.error);
  return parsed;
}

/** Runs in the worker, and in the page where there is no worker (a test, an old browser). */
export async function planHere(input: PlanInput): Promise<Day> {
  await init();
  return readAnswer(plan(JSON.stringify(input)));
}

let worker: Worker | null = null;
let next = 0;
const waiting = new Map<number, { resolve: (day: Day) => void; reject: (error: Error) => void }>();

function theWorker(): Worker {
  if (worker) return worker;
  worker = new Worker(new URL("./planner.worker.ts", import.meta.url), { type: "module" });
  worker.onmessage = (event: MessageEvent<{ id: number; day?: Day; error?: string }>) => {
    const entry = waiting.get(event.data.id);
    if (!entry) return;
    waiting.delete(event.data.id);
    if (event.data.day) entry.resolve(event.data.day);
    else entry.reject(new Error(event.data.error ?? "The planner did not answer."));
  };
  worker.onerror = () => {
    for (const entry of waiting.values()) entry.reject(new Error("The planner stopped. Reload the page."));
    waiting.clear();
    worker = null;
  };
  return worker;
}

/** The day, planned off the page's thread so the screen never stalls while it runs. */
export function computeDay(input: PlanInput): Promise<Day> {
  if (typeof Worker === "undefined") return planHere(input);
  const id = (next += 1);
  return new Promise<Day>((resolve, reject) => {
    waiting.set(id, { resolve, reject });
    theWorker().postMessage({ id, input });
  });
}
