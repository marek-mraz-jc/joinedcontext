/**
 * The rows of the praha-mesto space as this screen reads them (T-2917): nextbike docking
 * stations, park-and-ride car parks and ČHMÚ air-quality stations, each from the attributes the
 * praha-mesto model declares and nothing guessed. A row of another type, or one missing the
 * attribute a line is about, is left out rather than shown with a made-up zero.
 */
import type { RichCell, RichRow } from "@joinedcontext/sdk";

export interface BikeStation {
  id: string;
  name: string;
  bikes: number | null;
  docks: number | null;
  working: boolean;
  reportedAt?: string;
  /** `[longitude, latitude]` of a Point location; absent for any other shape. */
  at?: [number, number];
}

export interface CarPark {
  id: string;
  name: string;
  capacity: number | null;
  /** Free and occupied spaces right now: present only once the live counters feed the space. */
  free: number | null;
  occupied: number | null;
  reportedAt?: string;
}

/** One point of a map, coloured by `value`. */
export interface MapPoint {
  id: string;
  at: [number, number];
  value: number;
}

export type Pollutant = "pm10" | "pm25" | "no2" | "o3";
export const POLLUTANTS: readonly Pollutant[] = ["pm10", "pm25", "no2", "o3"];

export interface AirStation {
  id: string;
  name: string;
  readings: Partial<Record<Pollutant, { value: number; at?: string }>>;
}

function one(row: RichRow, attr: string): RichCell | undefined {
  const cell = row.cells[attr];
  return Array.isArray(cell) ? cell[0] : cell;
}

/** A count: a whole number that is not negative, or `null`. */
function count(cell: RichCell | undefined): number | null {
  const value = cell?.value;
  return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : null;
}

function measure(cell: RichCell | undefined): number | null {
  const value = cell?.value;
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

/** The coordinates of a Point location inside the WGS84 range, or `undefined`. */
function pointOf(row: RichRow): [number, number] | undefined {
  const geometry = one(row, "location")?.value as { type?: unknown; coordinates?: unknown } | undefined;
  if (geometry?.type !== "Point" || !Array.isArray(geometry.coordinates)) return undefined;
  const [lon, lat] = geometry.coordinates;
  return typeof lon === "number" && typeof lat === "number" && Math.abs(lon) <= 180 && Math.abs(lat) <= 90
    ? [lon, lat]
    : undefined;
}

function when(cell: RichCell | undefined): string | undefined {
  const value = cell?.value;
  return typeof value === "string" && !Number.isNaN(Date.parse(value)) ? value : undefined;
}

/** The name in `language`, else Czech, else any the publisher wrote; `null` when there is none. */
export function nameOf(row: RichRow, language: string): string | null {
  const cell = one(row, "name");
  const map = cell?.languageMap;
  const picked = map?.[language] ?? map?.cs ?? (map ? Object.values(map)[0] : undefined) ?? cell?.value;
  return typeof picked === "string" && picked.trim() !== "" ? picked.trim() : null;
}

export function toBikeStation(row: RichRow, language: string): BikeStation | null {
  if (row.type !== "BikeHireDockingStation") return null;
  const name = nameOf(row, language);
  if (!name) return null;
  return {
    id: row.id,
    name,
    bikes: count(one(row, "availableBikeNumber")),
    docks: count(one(row, "freeSlotNumber")),
    working: one(row, "status")?.value === "working",
    reportedAt: when(one(row, "dateModified")),
    at: pointOf(row),
  };
}

export function toCarPark(row: RichRow, language: string): CarPark | null {
  if (row.type !== "OffStreetParking") return null;
  const name = nameOf(row, language);
  if (!name) return null;
  return {
    id: row.id,
    name,
    capacity: count(one(row, "totalSpotNumber")),
    free: count(one(row, "availableSpotNumber")),
    occupied: count(one(row, "occupiedSpotNumber")),
    reportedAt: when(one(row, "dateModified")),
  };
}

export function toAirStation(row: RichRow, language: string): AirStation | null {
  if (row.type !== "AirQualityObserved") return null;
  const name = nameOf(row, language);
  if (!name) return null;
  const readings: AirStation["readings"] = {};
  for (const pollutant of POLLUTANTS) {
    const cell = one(row, pollutant);
    const value = measure(cell);
    if (value !== null) readings[pollutant] = { value, at: cell?.observedAt ?? when(one(row, "dateObserved")) };
  }
  return Object.keys(readings).length === 0 ? null : { id: row.id, name, readings };
}

export interface BikeTotals {
  stations: number;
  working: number;
  bikes: number;
  docks: number;
}

/** The totals over the stations that rent right now; a station out of service lends nothing. */
export function bikeTotals(stations: BikeStation[]): BikeTotals {
  const working = stations.filter((station) => station.working);
  return {
    stations: stations.length,
    working: working.length,
    bikes: working.reduce((sum, station) => sum + (station.bikes ?? 0), 0),
    docks: working.reduce((sum, station) => sum + (station.docks ?? 0), 0),
  };
}

/** Stations whose name holds `search`, case and diacritics ignored, by name. */
export function matching<T extends { name: string }>(rows: T[], search: string, locale: string): T[] {
  const fold = (text: string) => text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
  const wanted = fold(search.trim());
  return rows
    .filter((row) => wanted === "" || fold(row.name).includes(wanted))
    .sort((a, b) => a.name.localeCompare(b.name, locale));
}

/** The stations on the map, coloured by the bikes each holds; one without a location is left off. */
export function bikePoints(stations: BikeStation[]): MapPoint[] {
  return stations.flatMap((station) =>
    station.at && station.working && station.bikes !== null ? [{ id: station.id, at: station.at, value: station.bikes }] : [],
  );
}

/** The lower bound of each colour step of the bike map and histogram: 0, 1–2, 3–5, 6–10, 11+. */
export const BIKE_STEPS = [0, 1, 3, 6, 11];

/** How many stations in service hold a number of bikes inside each step. */
export function bikeHistogram(stations: BikeStation[], steps = BIKE_STEPS): number[] {
  const counts = steps.map(() => 0);
  for (const station of stations) {
    if (!station.working || station.bikes === null) continue;
    const bikes = station.bikes;
    const at = steps.findLastIndex((from) => bikes >= from);
    if (at >= 0) counts[at] += 1;
  }
  return counts;
}

/** `0`, `1–2`, …, `11+`: the label of each step. */
export function stepLabels(steps: number[]): string[] {
  return steps.map((from, i) => {
    const next = steps[i + 1];
    if (next === undefined) return `${from}+`;
    return next - 1 === from ? `${from}` : `${from}–${next - 1}`;
  });
}

function hex(colour: string): [number, number, number] | null {
  const match = /^#?([0-9a-f]{6})$/i.exec(colour.trim());
  if (!match) return null;
  const n = parseInt(match[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** The colour `t` (0..1) of the way from `low` to `high`; `high` when either is not a hex colour. */
export function between(low: string, high: string, t: number): string {
  const a = hex(low);
  const b = hex(high);
  if (!a || !b) return high;
  const c = a.map((v, i) => Math.round(v + (b[i] - v) * Math.min(1, Math.max(0, t))));
  return `#${c.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}

/** The histogram's bars, each in its step's legend colour, so a bar and the map dots of its step match. */
export function histogramBars(counts: number[], legend: { colour: string }[]): { value: number; itemStyle: { color: string } }[] {
  return counts.map((value, i) => ({ value, itemStyle: { color: legend[i]?.colour ?? legend[legend.length - 1]?.colour ?? "" } }));
}

/** One legend entry per step, from `low` to `high` on the branding ramp. */
export function legendOf(steps: number[], low: string, high: string): { from: number; label: string; colour: string }[] {
  const labels = stepLabels(steps);
  return steps.map((from, i) => ({
    from,
    label: labels[i],
    colour: between(low, high, steps.length === 1 ? 1 : i / (steps.length - 1)),
  }));
}
