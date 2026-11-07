import { format, pointOf } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";

/** The two entity types the app's data needs name (AP-04). */
export const AIR = "AirQualityObserved";
export const WEATHER = "WeatherObserved";

/** What the page compares, exactly the attributes app.yaml names. */
export const POLLUTANTS = ["pm10", "pm25", "airQualityIndex"] as const;
export const VARIABLES = ["temperature", "windSpeed", "relativeHumidity", "precipitation"] as const;
export type Pollutant = (typeof POLLUTANTS)[number];
export type Variable = (typeof VARIABLES)[number];

/** The attributes the station lists read. */
export const PLACE_ATTRS = ["name", "location", "dateObserved"];

export interface Station {
  id: string;
  local: string;
  name: string;
  at: [number, number] | null;
}

/** The end of an id: short enough for the address, unique in the city's feed. */
export function localOf(id: string): string {
  return id.slice(id.lastIndexOf(":") + 1);
}

/** The rows as stations, by name. */
export function stationsOf(rows: Row[]): Station[] {
  return rows
    .map((row) => ({ id: row.id, local: localOf(row.id), name: format(row.name).trim() || localOf(row.id), at: pointOf(row.location) }))
    .sort((a, b) => a.name.localeCompare(b.name, "fi") || a.id.localeCompare(b.id));
}

/** The great-circle distance between two `[lon, lat]` points, in kilometres. */
export function kmBetween(a: [number, number], b: [number, number]): number {
  const rad = Math.PI / 180;
  const dlat = (b[1] - a[1]) * rad;
  const dlon = (b[0] - a[0]) * rad;
  const h = Math.sin(dlat / 2) ** 2 + Math.cos(a[1] * rad) * Math.cos(b[1] * rad) * Math.sin(dlon / 2) ** 2;
  return 2 * 6371.0088 * Math.asin(Math.min(1, Math.sqrt(h)));
}

/** The weather station nearest `to`, with its distance; the first by name when places are missing. */
export function nearest(to: Station | undefined, weather: Station[]): { station: Station; km: number | null } | null {
  if (weather.length === 0) return null;
  if (!to?.at) return { station: weather[0], km: null };
  let best: { station: Station; km: number | null } = { station: weather[0], km: null };
  for (const station of weather) {
    if (!station.at) continue;
    const km = kmBetween(to.at, station.at);
    if (best.km === null || km < best.km) best = { station, km };
  }
  return best;
}
