/**
 * The App's server on the shared WASM host, `/apps/{name}/api` (T-3354): every quality run it has
 * kept with its scores, a run now, and a URL for a run's full per-entity report in the App's own
 * storage; who calls, the findings stewards resolved, a steward's correction of an attribute
 * through the App's Endpoint, and the history (T-3827). A test hands the page its own server
 * (`ServerContext`).
 */
import { createContext, useContext, useMemo } from "react";

export interface RunType {
  type: string;
  entities: number;
  completeness: number;
  /** `null` for a type without a published schema. */
  valid: number | null;
  findings: number;
  freshness_median_seconds: number | null;
}

export interface Run {
  id: number;
  ran_at: string;
  types: RunType[];
  entities: number;
  findings: number;
  completeness: number;
  valid: number | null;
}

export interface Runs {
  runs: Run[];
  /** The data could not be read just now: no new run, these are the runs as kept. */
  stale: boolean;
}

/** An answer of the server that is not a success, its `detail` as the message. */
export class Problem extends Error {
  constructor(
    readonly status: number,
    detail: string,
  ) {
    super(detail);
  }
}

/** Who is signed in, as the server names them; `null` when signed out. */
export interface Me {
  id: string;
  name: string;
  roles: string[];
}

/** A finding a steward resolved: marked, or corrected through the Endpoint. */
export interface Resolution {
  id: number;
  entity: string;
  attribute: string;
  note: string;
  corrected: boolean;
  resolverName: string;
  resolvedAt: string;
}

export interface HistoryRow {
  id: number;
  at: string;
  actorName: string;
  action: "insert" | "update" | "delete" | "edit";
  target: string;
  summary: string;
}

export interface Server {
  me(): Promise<Me | null>;
  resolutions(): Promise<Resolution[]>;
  resolve(entity: string, attribute: string, note: string): Promise<Resolution>;
  reopen(id: number): Promise<void>;
  /** The attribute written through the Endpoint; the new resolution, or `null` when one stood. */
  correct(entity: string, attribute: string, value: string | number | boolean, note: string): Promise<Resolution | null>;
  history(): Promise<HistoryRow[]>;
  runs(): Promise<Runs>;
  /** A URL the run's full report can be downloaded from for two minutes. */
  reportUrl(id: number): Promise<string>;
}

/** Where the server answers: `/apps/{appName}/api`, or `/api` when the name is not an App's. */
export function apiBase(appName: string | undefined): string {
  return appName && /^[a-z0-9-]+$/.test(appName) ? `/apps/${appName}/api` : "/api";
}

async function call<T>(url: string, method = "GET", body?: unknown): Promise<T> {
  let answer: Response;
  try {
    answer = await fetch(url, {
      method,
      credentials: "same-origin",
      headers: body === undefined ? { Accept: "application/json" } : { Accept: "application/json", "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new Problem(0, "the server could not be reached");
  }
  if (!answer.ok) {
    const problem = (await answer.json().catch(() => ({}))) as { detail?: unknown };
    throw new Problem(answer.status, typeof problem.detail === "string" ? problem.detail : `HTTP ${answer.status}`);
  }
  return (answer.status === 204 ? undefined : await answer.json()) as T;
}

export function server(base: string): Server {
  return {
    runs: () => call<Runs>(`${base}/runs`),
    reportUrl: async (id) => (await call<{ url: string }>(`${base}/runs/${id}/report`)).url,
    me: () => call<Me | null>(`${base}/me`),
    resolutions: () => call<Resolution[]>(`${base}/resolutions`),
    resolve: (entity, attribute, note) => call<Resolution>(`${base}/resolutions`, "POST", { entity, attribute, note }),
    reopen: (id) => call<void>(`${base}/resolutions/${id}`, "DELETE"),
    correct: async (entity, attribute, value, note) => {
      const answer = await call<Resolution | { corrected: true; id?: undefined }>(`${base}/corrections`, "POST", { entity, attribute, value, note });
      return answer.id === undefined ? null : answer;
    },
    history: () => call<HistoryRow[]>(`${base}/history`),
  };
}

/** The server the page talks to; `null` takes the App's own, from its name. */
export const ServerContext = createContext<Server | null>(null);

export function useServer(appName: string | undefined): Server {
  const given = useContext(ServerContext);
  return useMemo(() => given ?? server(apiBase(appName)), [given, appName]);
}
