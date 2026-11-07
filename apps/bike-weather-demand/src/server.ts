/**
 * The App's server on the shared WASM host, `/apps/{name}/api` (T-3355): the models it has kept of
 * a station, one a day, and a URL for the data a model was trained on, in the App's own storage;
 * who reads, a station's model inputs a steward tunes, and the history of changes (T-3827). A test
 * hands the page its own server (`ServerContext`).
 */
import { createContext, useContext, useMemo } from "react";

export interface Model {
  id: number;
  /** `YYYY-MM-DD`, the day on Helsinki's calendar. */
  trained_on: string;
  trained_at: string;
  hours: number;
  enough: boolean;
  /** Bikes per degree, and with rain; `null` without weather terms. */
  per_degree: number | null;
  rain: number | null;
  sigma: number | null;
  weather_station: string | null;
  /** The days of history it trained on; `null` for a model kept before a steward could set them. */
  days?: number | null;
}

/** Who reads, as the server's `identity::caller()` says; roles from the token, never the page. */
export interface Me {
  id: string;
  name: string;
  roles: string[];
}

/** What a steward sets for a station's model. */
export interface NewInputs {
  /** A weather station instead of the nearest; `null` for the nearest within the radius. */
  weatherStation: string | null;
  radiusKm: number;
  days: number;
  note: string;
}

/** A station's model inputs: a steward's, else the defaults (`custom: false`). */
export interface Inputs extends NewInputs {
  station: string;
  custom: boolean;
  updatedBy: string | null;
  updatedAt: string | null;
}

/** One change, from the App's `app_audit` table. */
export interface HistoryRow {
  id: number;
  at: string;
  actorName: string;
  action: "insert" | "update" | "delete" | "edit";
  target: string;
  summary: string;
}

export interface Models {
  models: Model[];
  /** The data could not be read just now: no model today, these are the models as kept. */
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

export interface Server {
  models(station: string): Promise<Models>;
  /** A URL the model's training data can be downloaded from for two minutes. */
  snapshotUrl(id: number): Promise<string>;
  /** Who is signed in; `null` when nobody is. */
  me(): Promise<Me | null>;
  inputs(station: string): Promise<Inputs>;
  /** A steward's; today's model is trained again on them. */
  setInputs(station: string, inputs: NewInputs): Promise<Inputs>;
  /** A steward's: back to the defaults. */
  resetInputs(station: string): Promise<void>;
  history(): Promise<HistoryRow[]>;
}

/** Where the server answers: `/apps/{appName}/api`, or `/api` when the name is not an App's. */
export function apiBase(appName: string | undefined): string {
  return appName && /^[a-z0-9-]+$/.test(appName) ? `/apps/${appName}/api` : "/api";
}

async function call<T>(url: string, method = "GET", body?: unknown): Promise<T> {
  const answer = await fetch(url, {
    method,
    credentials: "same-origin",
    headers: body === undefined ? { Accept: "application/json" } : { Accept: "application/json", "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!answer.ok) {
    const problem = (await answer.json().catch(() => ({}))) as { detail?: unknown };
    throw new Problem(answer.status, typeof problem.detail === "string" ? problem.detail : `HTTP ${answer.status}`);
  }
  return (answer.status === 204 ? undefined : await answer.json()) as T;
}

export function server(base: string): Server {
  const at = (station: string) => `${base}/inputs?station=${encodeURIComponent(station)}`;
  return {
    models: (station) => call<Models>(`${base}/models?station=${encodeURIComponent(station)}`),
    snapshotUrl: async (id) => (await call<{ url: string }>(`${base}/models/${id}/snapshot`)).url,
    me: () => call<Me | null>(`${base}/me`),
    inputs: (station) => call<Inputs>(at(station)),
    setInputs: (station, inputs) => call<Inputs>(at(station), "PUT", inputs),
    resetInputs: (station) => call<void>(at(station), "DELETE"),
    history: () => call<HistoryRow[]>(`${base}/history`),
  };
}

/** The server the page talks to; `null` takes the App's own, from its name. */
export const ServerContext = createContext<Server | null>(null);

export function useServer(appName: string | undefined): Server {
  const given = useContext(ServerContext);
  return useMemo(() => given ?? server(apiBase(appName)), [given, appName]);
}
