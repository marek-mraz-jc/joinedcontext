/**
 * The App's server, `/apps/transit-reach/api` (T-3349): HSL's network kept by version, and from a
 * stop the area reached in each band, computed once per version and kept with its GeoJSON, which
 * the browser downloads from the store through a short-lived URL the server hands out. A signed-in
 * visitor keeps origins with their areas to compare; a steward publishes a shared set (T-3826).
 */
import { ServerProblem } from "./problem";

export { ServerProblem };

export interface KeptBand {
  minutes: number;
  areaKm2: number;
  stops: number;
}

export interface KeptReach {
  version: string;
  stop: string;
  bands: KeptBand[];
  /** Whether it was computed before, on this version of the network. */
  cached: boolean;
  /** Whether the network is due to be read again. */
  stale: boolean;
  url: string;
}

/** The signed-in person as the App's server knows them, `null` when signed out. */
export interface Me {
  id: string;
  name: string;
  roles: string[];
}

/** An origin as the page sends it: a name, the point, and the areas reached from it. */
export interface NewOrigin {
  name: string;
  lon: number;
  lat: number;
  source: "network" | "derived";
  bands: KeptBand[];
}

/** An origin the server keeps: the caller's own, a published one, or (for a steward) anyone's. */
export interface Origin extends NewOrigin {
  id: number;
  published: boolean;
  mine: boolean;
  createdAt: string;
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

/** Where the server answers: `/apps/{appName}/api`, the App's name from `#jc-config`. */
export function apiBase(doc: Document = document): string {
  const text = doc.getElementById("jc-config")?.textContent ?? "";
  let name = "";
  try {
    name = String((JSON.parse(text || "{}") as { appName?: unknown }).appName ?? "");
  } catch {
    name = "";
  }
  return `/apps/${name && /^[a-z0-9-]+$/.test(name) ? name : "transit-reach"}/api`;
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

export function reachApi(base: string) {
  return {
    reach: (stop: string) => call<KeptReach>(base, "GET", `/reach?${new URLSearchParams({ stop }).toString()}`),
    refresh: () => call<{ version: string; stops: number; routes: number; changed: boolean }>(base, "POST", "/network"),
    me: () => call<Me | null>(base, "GET", "/me"),
    origins: () => call<Origin[]>(base, "GET", "/origins"),
    keep: (origin: NewOrigin) => call<Origin>(base, "POST", "/origins", origin),
    rename: (id: number, name: string) => call<void>(base, "PUT", `/origins/${id}`, { name }),
    forget: (id: number) => call<void>(base, "DELETE", `/origins/${id}`),
    publish: (id: number, published: boolean) => call<void>(base, "PUT", `/origins/${id}/published`, { published }),
    history: () => call<HistoryRow[]>(base, "GET", "/history"),
  };
}

export type ReachApi = ReturnType<typeof reachApi>;

/**
 * The kept areas from `stop`: when the server has not read HSL's network yet (409) it is asked to,
 * once, and then for the areas again; a network due to be read again is read in the background.
 */
export async function keptReach(api: Pick<ReachApi, "reach" | "refresh">, stop: string): Promise<KeptReach> {
  let kept: KeptReach;
  try {
    kept = await api.reach(stop);
  } catch (error) {
    if (!(error instanceof ServerProblem) || error.status !== 409) throw error;
    await api.refresh();
    kept = await api.reach(stop);
  }
  if (kept.stale) api.refresh().catch(() => undefined);
  return kept;
}
