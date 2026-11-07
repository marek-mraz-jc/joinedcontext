import init, { plan } from "../wasm/pkg/bike_rebalancing.js";

/** A docking station as the planner reads it (wasm/src/lib.rs `Station`). */
export interface Station {
  id: string;
  name: string;
  lon: number | null;
  lat: number | null;
  bikes: number | null;
  free: number | null;
  capacity: number | null;
}

export interface Settings {
  low?: number;
  high?: number;
  target?: number;
  vanCapacity?: number;
  start?: [number, number] | null;
  include?: string[];
  exclude?: string[];
}

export type Level = "empty" | "low" | "balanced" | "high" | "full" | "unknown";

export interface Need {
  id: string;
  level: Level;
  fill: number | null;
  surplus: number;
}

export interface Stop {
  id: string;
  name: string;
  at: [number, number];
  action: "pick" | "drop";
  bikes: number;
  load: number;
  legKm: number;
}

export interface Plan {
  needs: Need[];
  route: { stops: Stop[]; km: number; moved: number };
}

export interface PlanInput {
  stations: Station[];
  settings: Settings;
}

/** What the module answers: the plan, or the sentence of what it could not read. */
export function readAnswer(answer: string): Plan {
  const parsed = JSON.parse(answer) as Plan | { error: string };
  if ("error" in parsed) throw new Error(parsed.error);
  return parsed;
}

/** Runs in the worker, and in the page where there is no worker (a test, an old browser). */
export async function planHere(input: PlanInput): Promise<Plan> {
  await init();
  return readAnswer(plan(JSON.stringify(input)));
}

let worker: Worker | null = null;
let next = 0;
const waiting = new Map<number, { resolve: (plan: Plan) => void; reject: (error: Error) => void }>();

function theWorker(): Worker {
  if (worker) return worker;
  worker = new Worker(new URL("./planner.worker.ts", import.meta.url), { type: "module" });
  worker.onmessage = (event: MessageEvent<{ id: number; plan?: Plan; error?: string }>) => {
    const entry = waiting.get(event.data.id);
    if (!entry) return;
    waiting.delete(event.data.id);
    if (event.data.plan) entry.resolve(event.data.plan);
    else entry.reject(new Error(event.data.error ?? "The planner did not answer."));
  };
  worker.onerror = () => {
    for (const entry of waiting.values()) entry.reject(new Error("The planner stopped. Reload the page."));
    waiting.clear();
    worker = null;
  };
  return worker;
}

/** The plan, computed off the page's thread so the screen never stalls while it runs. */
export function computePlan(input: PlanInput): Promise<Plan> {
  if (typeof Worker === "undefined") return planHere(input);
  const id = (next += 1);
  return new Promise<Plan>((resolve, reject) => {
    waiting.set(id, { resolve, reject });
    theWorker().postMessage({ id, input });
  });
}
