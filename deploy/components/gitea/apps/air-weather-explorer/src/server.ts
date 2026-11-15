/**
 * The App's server, `/apps/air-weather-explorer/api` (T-3348): each station's hourly means, kept
 * on the server and read from the city's data only for the hours it has not kept yet; comparisons
 * saved under a code a link carries; and CSV exports, downloaded from the store through a
 * short-lived URL the server hands out.
 */
import { ServerProblem } from "./problem";
import type { Pollutant, Variable } from "./stations";

export { ServerProblem };

/** Each attribute's hourly means, `[hour start ms, value]`. */
export type Hourly = Record<string, Array<[number, number]>>;

export interface Series {
  air: Hourly;
  weather: Hourly;
}

export interface Comparison {
  name: string | null;
  /** The stations' full ids. */
  station: string;
  weather: string;
  days: number;
  smoothing: number;
  air: Pollutant | null;
  variable: Variable | null;
}

/** The signed-in person as the App's server knows them, `null` when signed out. */
export interface Me {
  id: string;
  name: string;
  roles: string[];
}

/** A place a reader keeps: an air quality station, a pollutant, the level that alerts them. */
export interface NewPlace {
  station: string;
  name: string;
  attr: "pm10" | "pm25" | "airQualityIndex";
  threshold: number;
}

/** A kept place, with the latest hourly mean the server holds for it (`null` before any). */
export interface Place extends NewPlace {
  id: number;
  latest: number | null;
  latestHour: number | null;
}

/** A period of a station's readings a steward marked faulty, in epoch milliseconds. */
export interface Fault {
  id: number;
  station: string;
  from: number;
  to: number;
  note: string;
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
  return `/apps/${name && /^[a-z0-9-]+$/.test(name) ? name : "air-weather-explorer"}/api`;
}

/** A code as the server makes them: 12 lowercase letters or digits. */
export function isCode(code: string): boolean {
  return /^[a-z0-9]{12}$/.test(code);
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

export function airApi(base: string) {
  return {
    series: (air: string, weather: string, days: number) =>
      call<Series>(base, "GET", `/series?${new URLSearchParams({ air, weather, days: String(days) }).toString()}`),
    save: (comparison: Comparison) => call<{ code: string; name: string | null }>(base, "POST", "/comparisons", comparison),
    open: (code: string) => call<Comparison & { code: string }>(base, "GET", `/comparisons/${encodeURIComponent(code)}`),
    exportUrl: async (air: string, weather: string, days: number) => (await call<{ url: string }>(base, "POST", "/exports", { air, weather, days })).url,
    me: () => call<Me | null>(base, "GET", "/me"),
    places: () => call<Place[]>(base, "GET", "/places"),
    addPlace: (place: NewPlace) => call<Place>(base, "POST", "/places", place),
    changePlace: (id: number, name: string, threshold: number) => call<void>(base, "PUT", `/places/${id}`, { name, threshold }),
    removePlace: (id: number) => call<void>(base, "DELETE", `/places/${id}`),
    faults: (station: string) => call<Fault[]>(base, "GET", `/faults?${new URLSearchParams({ station }).toString()}`),
    markFault: (fault: Omit<Fault, "id">) => call<Fault>(base, "POST", "/faults", fault),
    unmarkFault: (id: number) => call<void>(base, "DELETE", `/faults/${id}`),
    history: () => call<HistoryRow[]>(base, "GET", "/history"),
  };
}

export type AirApi = ReturnType<typeof airApi>;

/** The address that opens a saved comparison: this page with `?compare=` and nothing else. */
export function compareLink(code: string, location: Pick<Location, "origin" | "pathname"> = window.location): string {
  return `${location.origin}${location.pathname}?compare=${code}`;
}
