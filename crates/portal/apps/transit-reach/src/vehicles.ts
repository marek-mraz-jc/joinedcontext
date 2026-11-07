/**
 * The vehicles as the page reads them: their positions, speeds and routes over the last hours from
 * the temporal read, and the view in the address (`?at=24.9414,60.1710&hours=3`).
 */
import type { TemporalPoint, TemporalRow } from "@joinedcontext/sdk";
import type { VehicleInput } from "./analysis";

export const VEHICLE = "Vehicle";
/** What the Vehicle entities carry on dev (2026-10-08): where, how fast, on which route. */
export const ATTRS = ["location", "speed", "route"];
/** The history windows offered, in hours; the App's grant reads at most the longest (temporalQ PT6H). */
export const HOURS = [1, 3, 6] as const;
export type Hours = (typeof HOURS)[number];
export const DEFAULT_HOURS: Hours = 3;
/** The most instances the temporal read asks for per vehicle. */
export const LAST_N = 1000;
/** Where the page starts: the Central Railway Station's square, Rautatientori. */
export const CENTRE = { lon: 24.9414, lat: 60.171 };

export interface ViewState {
  at: { lon: number; lat: number };
  hours: Hours;
}

export const EMPTY: ViewState = { at: CENTRE, hours: DEFAULT_HOURS };

/** A point in the capital region from `lon,lat`, or `null`. */
export function pointOf(text: string | null): { lon: number; lat: number } | null {
  const [lon, lat, ...rest] = (text ?? "").split(",").map((part) => Number(part.trim()));
  if (rest.length > 0 || !Number.isFinite(lon) || !Number.isFinite(lat)) return null;
  // The capital region and a margin: a point elsewhere has no transit here to reach with.
  return lon >= 24 && lon <= 26 && lat >= 59.9 && lat <= 60.6 ? { lon, lat } : null;
}

export function readView(search: string): ViewState {
  const params = new URLSearchParams(search);
  const hours = Number(params.get("hours"));
  return {
    at: pointOf(params.get("at")) ?? CENTRE,
    hours: (HOURS as readonly number[]).includes(hours) ? (hours as Hours) : DEFAULT_HOURS,
  };
}

/** The address for a view, every other parameter (the language) kept; defaults are left out. */
export function writeView(search: string, view: ViewState): string {
  const params = new URLSearchParams(search);
  params.delete("at");
  params.delete("hours");
  if (view.at.lon !== CENTRE.lon || view.at.lat !== CENTRE.lat) params.set("at", `${view.at.lon.toFixed(5)},${view.at.lat.toFixed(5)}`);
  if (view.hours !== DEFAULT_HOURS) params.set("hours", String(view.hours));
  const text = params.toString();
  return text ? `?${text}` : "";
}

const timeOf = (point: TemporalPoint) => Date.parse(point.observedAt);

/**
 * Each vehicle's readings: a position with its time, the speed read at that time, and the route the
 * vehicle last named at or before it (a route is re-sent with every update, but need not be).
 */
export function toVehicles(history: TemporalRow[]): VehicleInput[] {
  return history.map((row) => {
    const speeds = new Map<number, number>();
    for (const point of row.series.speed ?? []) {
      if (typeof point.value === "number") speeds.set(timeOf(point), point.value);
    }
    const routes = (row.series.route ?? [])
      .map((point) => ({ t: timeOf(point), route: point.value === null ? "" : String(point.value) }))
      .filter((r) => Number.isFinite(r.t))
      .sort((a, b) => a.t - b.t);
    const located = (row.series.location ?? [])
      .map((point) => ({ t: timeOf(point), geo: point.value }))
      .filter(({ t }) => Number.isFinite(t))
      .sort((a, b) => a.t - b.t);
    const points = [];
    let next = 0;
    let route = "";
    for (const { t, geo } of located) {
      // Both lists in time order: the route in force is the last one named at or before t.
      while (next < routes.length && routes[next].t <= t) route = routes[next++].route;
      if (geo === null || typeof geo !== "object" || geo.type !== "Point" || !Array.isArray(geo.coordinates)) continue;
      const [lon, lat] = geo.coordinates as unknown[];
      if (typeof lon !== "number" || typeof lat !== "number") continue;
      const speed = speeds.get(t);
      points.push({ t, lon, lat, ...(speed !== undefined ? { speed } : {}), ...(route ? { route } : {}) });
    }
    return { id: row.id, points };
  });
}
