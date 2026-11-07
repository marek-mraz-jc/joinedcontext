/**
 * The App's server on the shared WASM host, `/apps/{name}/api` (T-3352): the topics of every
 * week it has kept, and a URL for each week's articles as a file in the App's own storage. A
 * reader follows topics by their top keyword; a steward names and merges topics, which everyone
 * reads (T-3828). A test hands the page its own server (`ServerContext`).
 */
import { createContext, useContext, useMemo } from "react";

export interface KeptKeyword {
  term: string;
  weight: number;
}

export interface KeptTopic {
  topic: number;
  share: number;
  articles: number;
  keywords: KeptKeyword[];
}

export interface KeptWeek {
  /** The ISO week, `2026-W41`. */
  week: string;
  articles: number;
  computed_at: string;
  topics: KeptTopic[];
}

export interface Kept {
  weeks: KeptWeek[];
  /** The feed could not be read just now: these are the weeks as last kept. */
  stale: boolean;
}

/** Who is signed in, as the App's server knows them. */
export interface Me {
  id: string;
  name: string;
  roles: string[];
}

/** A keyword the reader follows: a topic, found again by its top keyword. */
export interface Follow {
  id: number;
  term: string;
}

/** A steward's name for the topic of a top keyword. */
export interface TopicName {
  id: number;
  term: string;
  name: string;
  ownerName: string;
}

/** One line of the App's history (`app_audit`). */
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
  weeks(): Promise<Kept>;
  /** A URL the week's articles can be downloaded from for two minutes. */
  corpusUrl(week: string): Promise<string>;
  /** Who is signed in; `null` when nobody is. */
  me(): Promise<Me | null>;
  /** The reader's followed keywords, newest first. */
  follows(): Promise<Follow[]>;
  follow(term: string): Promise<Follow>;
  unfollow(id: number): Promise<void>;
  /** Every topic name a steward gave. */
  names(): Promise<TopicName[]>;
  /** Names, or renames, the topic of `term`. */
  name(term: string, name: string): Promise<TopicName>;
  unname(id: number): Promise<void>;
  /** The newest changes: a steward's everyone's, anyone else's their own. */
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
    weeks: () => get<Kept>(`${base}/weeks`),
    corpusUrl: async (week) => (await get<{ url: string }>(`${base}/weeks/${encodeURIComponent(week)}/corpus`)).url,
    me: () => get<Me | null>(`${base}/me`),
    follows: () => get<Follow[]>(`${base}/follows`),
    follow: (term) => get<Follow>(`${base}/follows`, "POST", { term }),
    unfollow: (id) => get<void>(`${base}/follows/${id}`, "DELETE"),
    names: () => get<TopicName[]>(`${base}/names`),
    name: (term, name) => get<TopicName>(`${base}/names`, "POST", { term, name }),
    unname: (id) => get<void>(`${base}/names/${id}`, "DELETE"),
    changes: () => get<Change[]>(`${base}/history`),
  };
}

/** The server the page talks to; `null` takes the App's own, from its name. */
export const ServerContext = createContext<Server | null>(null);

export function useServer(appName: string | undefined): Server {
  const given = useContext(ServerContext);
  return useMemo(() => given ?? server(apiBase(appName)), [given, appName]);
}
