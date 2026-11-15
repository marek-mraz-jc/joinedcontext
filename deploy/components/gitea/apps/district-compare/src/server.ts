/**
 * The App's server on the shared WASM host, `/apps/{name}/api` (T-3353): the figures it keeps of
 * every district for every day, and URLs for the district boundaries it compared against and
 * their licence, as files in the App's own storage; who reads, the comparisons a reader keeps and
 * shares, the one a steward pins for everyone, and the history of changes (T-3828). A test hands
 * the page its own server (`ServerContext`).
 */
import { createContext, useContext, useMemo } from "react";
import type { Measure } from "./districts";

/** One district's figures on one day on Helsinki's calendar. */
export interface DayMetric {
  /** `YYYY-MM-DD`. */
  day: string;
  code: string;
  name: string;
  area_km2: number;
  events: number;
  bikes: number;
  bike_slots: number;
  alerts: number;
  pm25: number | null;
  aqi: number | null;
}

export interface History {
  days: DayMetric[];
  /** The feeds could not be read just now: today is missing, these are the days as last kept. */
  stale: boolean;
}

export interface Files {
  geojson: string;
  licence: string;
}

/** Who reads, as the server's `identity::caller()` says; roles from the token, never the page. */
export interface Me {
  id: string;
  name: string;
  roles: string[];
}

/** What a reader keeps: a name, up to six district codes and a measure. */
export interface NewComparison {
  name: string;
  codes: string[];
  measure: Measure;
}

export interface Comparison extends NewComparison {
  id: number;
  ownerName: string;
  createdAt?: string;
  /** On a shared comparison: whether the reader owns it. */
  mine?: boolean;
  /** The owner's own comparison: the token its shared link carries (64 hex characters). */
  share?: string;
}

/** One change, from the App's `app_audit` table. */
export interface Change {
  id: number;
  at: string;
  actorName: string;
  action: "insert" | "update" | "delete" | "edit";
  target: string;
  summary: string;
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
  /** The kept days of up to six districts, the newest first. */
  history(codes: string[]): Promise<History>;
  /** URLs the boundaries and their licence can be downloaded from for two minutes. */
  files(): Promise<Files>;
  /** Who is signed in; `null` when nobody is. */
  me(): Promise<Me | null>;
  /** The reader's own comparisons, newest first. */
  comparisons(): Promise<Comparison[]>;
  keep(comparison: NewComparison): Promise<Comparison>;
  /** Any comparison, read-only, by the token a shared link carries; never by its id. */
  shared(token: string): Promise<Comparison>;
  change(id: number, comparison: NewComparison): Promise<void>;
  forget(id: number): Promise<void>;
  /** The comparison a steward pinned for everyone; `null` when none. */
  pinned(): Promise<Comparison | null>;
  pin(comparison: NewComparison): Promise<Comparison>;
  unpin(): Promise<void>;
  changes(): Promise<Change[]>;
}

/** Where the server answers: `/apps/{appName}/api`, or `/api` when the name is not an App's. */
export function apiBase(appName: string | undefined): string {
  return appName && /^[a-z0-9-]+$/.test(appName) ? `/apps/${appName}/api` : "/api";
}

async function get<T>(url: string, method = "GET", body?: unknown): Promise<T> {
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
  return {
    history: (codes) => get<History>(`${base}/metrics?codes=${encodeURIComponent(codes.join(","))}`),
    files: () => get<Files>(`${base}/boundaries`),
    me: () => get<Me | null>(`${base}/me`),
    comparisons: () => get<Comparison[]>(`${base}/comparisons`),
    keep: (comparison) => get<Comparison>(`${base}/comparisons`, "POST", comparison),
    shared: (token) => get<Comparison>(`${base}/shared/${encodeURIComponent(token)}`),
    change: (id, comparison) => get<void>(`${base}/comparisons/${id}`, "PUT", comparison),
    forget: (id) => get<void>(`${base}/comparisons/${id}`, "DELETE"),
    pinned: () => get<Comparison | null>(`${base}/default`),
    pin: (comparison) => get<Comparison>(`${base}/default`, "PUT", comparison),
    unpin: () => get<void>(`${base}/default`, "DELETE"),
    changes: () => get<Change[]>(`${base}/history`),
  };
}

/** The server the page talks to; `null` takes the App's own, from its name. */
export const ServerContext = createContext<Server | null>(null);

export function useServer(appName: string | undefined): Server {
  const given = useContext(ServerContext);
  return useMemo(() => given ?? server(apiBase(appName)), [given, appName]);
}
