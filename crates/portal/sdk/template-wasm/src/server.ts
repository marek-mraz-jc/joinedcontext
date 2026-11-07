/**
 * The App's own server on the shared WASM host, `/apps/{appName}/api` (ADR-N-044): the items it
 * keeps in its own table. A test hands the page its own server through `ServerContext`.
 */
import { createContext, useContext, useMemo } from "react";

export interface Item {
  id: number;
  text: string;
  created_at: string;
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
  items(): Promise<Item[]>;
  addItem(text: string): Promise<Item>;
}

/** Where the server answers: `/apps/{appName}/api`, or `/api` when the name is not an App's. */
export function apiBase(appName: string | undefined): string {
  return appName && /^[a-z0-9-]+$/.test(appName) ? `/apps/${appName}/api` : "/api";
}

async function call<T>(url: string, init: RequestInit = {}): Promise<T> {
  const answer = await fetch(url, {
    ...init,
    credentials: "same-origin",
    headers: { Accept: "application/json", ...(init.body ? { "Content-Type": "application/json" } : {}) },
  });
  if (!answer.ok) {
    const problem = (await answer.json().catch(() => ({}))) as { detail?: unknown };
    throw new Problem(answer.status, typeof problem.detail === "string" ? problem.detail : `HTTP ${answer.status}`);
  }
  return (await answer.json()) as T;
}

export function server(base: string): Server {
  return {
    items: () => call<Item[]>(`${base}/items`),
    addItem: (text) => call<Item>(`${base}/items`, { method: "POST", body: JSON.stringify({ text }) }),
  };
}

/** The server the page talks to; `null` takes the App's own, from its name. */
export const ServerContext = createContext<Server | null>(null);

export function useServer(appName: string | undefined): Server {
  const given = useContext(ServerContext);
  return useMemo(() => given ?? server(apiBase(appName)), [given, appName]);
}
