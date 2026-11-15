/**
 * The App's server, `/apps/bike-rebalancing/api` (T-3346): plans saved with the counts of the
 * moment, the drives made on them, and each plan's route sheet, which the browser downloads from
 * the store through a short-lived URL the server hands out.
 */
import type { Stop } from "./planner";

export interface PlanSummary {
  id: number;
  operator: string;
  vanCapacity: number;
  km: number;
  moved: number;
  stopCount: number;
  createdAt: string;
  drives: number;
  /** The caller saved it; a steward also sees everyone else's. */
  mine: boolean;
}

export interface DriveRecord {
  id: number;
  stops: string[];
  km: number | null;
  note: string | null;
  drivenAt: string;
}

export interface SavedPlan {
  id: number;
  operator: string;
  vanCapacity: number;
  start: string | null;
  include: string[];
  exclude: string[];
  stops: Stop[];
  km: number;
  moved: number;
  createdAt: string;
  drives: DriveRecord[];
}

/** The signed-in person as the App's server knows them, `null` when signed out. */
export interface Me {
  id: string;
  name: string;
  roles: string[];
}

/** The planner's shares of a station's places, one set for every plan. */
export interface Levels {
  low: number;
  high: number;
  target: number;
}

/** One change as the history keeps it. */
export interface HistoryRow {
  id: number;
  at: string;
  actorName: string;
  action: "insert" | "update" | "delete" | "edit";
  target: string;
  summary: string;
}

export interface NewPlan {
  operator: string;
  vanCapacity: number;
  start: string | null;
  include: string[];
  exclude: string[];
}

/** A problem+json answer as an error whose message is its `detail`. */
export class ServerProblem extends Error {
  constructor(
    readonly status: number,
    detail: string,
  ) {
    super(detail);
  }
}

/** Where the server answers: `/apps/{appName}/api`, the App's name from `#jc-config`. */
export function apiBase(doc: Document = document): string {
  const text = doc.getElementById("jc-config")?.textContent ?? "";
  let name = "";
  try {
    name = String((JSON.parse(text || "{}") as { appName?: unknown }).appName ?? "");
  } catch {
    name = "";
  }
  return `/apps/${name && /^[a-z0-9-]+$/.test(name) ? name : "bike-rebalancing"}/api`;
}

async function call<T>(base: string, method: string, path: string, body?: unknown): Promise<T> {
  let answer: Response;
  try {
    answer = await fetch(`${base}${path}`, {
      method,
      credentials: "same-origin",
      headers: body === undefined ? { Accept: "application/json" } : { Accept: "application/json", "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new ServerProblem(0, "");
  }
  if (!answer.ok) {
    const problem = (await answer.json().catch(() => ({}))) as { detail?: unknown };
    throw new ServerProblem(answer.status, typeof problem.detail === "string" ? problem.detail : "");
  }
  return (answer.status === 204 ? undefined : await answer.json()) as T;
}

export function plansApi(base: string) {
  return {
    list: (operator: string) => call<PlanSummary[]>(base, "GET", `/plans${operator.trim() ? `?operator=${encodeURIComponent(operator.trim())}` : ""}`),
    save: (plan: NewPlan) => call<SavedPlan>(base, "POST", "/plans", plan),
    get: (id: number) => call<SavedPlan>(base, "GET", `/plans/${id}`),
    remove: (id: number) => call<void>(base, "DELETE", `/plans/${id}`),
    drive: (id: number, stops: string[]) => call<DriveRecord>(base, "POST", `/plans/${id}/drives`, { stops }),
    sheetUrl: async (id: number) => (await call<{ url: string }>(base, "GET", `/plans/${id}/sheet`)).url,
    handOver: (id: number, operator: string) => call<void>(base, "PUT", `/plans/${id}/operator`, { operator }),
    me: () => call<Me | null>(base, "GET", "/me"),
    levels: () => call<Levels>(base, "GET", "/levels"),
    setLevels: (levels: Levels) => call<Levels>(base, "PUT", "/levels", levels),
    history: () => call<HistoryRow[]>(base, "GET", "/history"),
  };
}

export type PlansApi = ReturnType<typeof plansApi>;
