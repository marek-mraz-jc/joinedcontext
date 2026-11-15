/**
 * Types and helper functions for city-bike station history, weather alignment,
 * spatial nearest-station selection, hash serialization, and WASM estimation input.
 */
import type { Cell, Row, TemporalPoint } from "@joinedcontext/sdk";
import { format, pointOf } from "@joinedcontext/sdk";

export interface BikeHistoryPoint {
  at: string;
  value: number;
}

export interface WeatherHistoryPoint {
  at: string;
  temperature: number | null;
  precipitation: number | null;
}

export interface EstimateInput {
  now: number;
  totalSlots: number;
  bikes: BikeHistoryPoint[];
  weather: WeatherHistoryPoint[];
}

export interface ProfileSlot {
  slot: number; // 0..167 (Monday 00:00 to Sunday 23:00 Europe/Helsinki)
  mean: number | null;
  count: number;
}

export interface WeatherEffect {
  perDegree: number;
  rain: number;
  hours: number;
}

export interface HourlyEstimate {
  at: string;
  mean: number;
  low: number;
  high: number;
}

export interface WeatherAssumption {
  temperature: number;
  raining: boolean;
}

export interface EstimateOutput {
  hours: number;
  profile: ProfileSlot[];
  weather: WeatherEffect | null;
  sigma: number | null;
  estimate: HourlyEstimate[];
  assumption: WeatherAssumption | null;
  enough: boolean;
}

/** Great-circle distance between two longitude/latitude points in kilometres. */
export function haversineKm(lon1: number, lat1: number, lon2: number, lat2: number): number {
  const dLat = ((lat2 - lat1) * Math.PI) / 180;
  const dLon = ((lon2 - lon1) * Math.PI) / 180;
  const a =
    Math.sin(dLat / 2) * Math.sin(dLat / 2) +
    Math.cos((lat1 * Math.PI) / 180) * Math.cos((lat2 * Math.PI) / 180) * Math.sin(dLon / 2) * Math.sin(dLon / 2);
  const c = 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));
  return 6371 * c;
}

/** Finds the weather station nearest to the given bike station within maxDistanceKm (default 10 km). */
export function nearestWeatherStation(
  bikeStation: Row,
  weatherStations: Row[],
  maxDistanceKm = 10,
): { station: Row; distanceKm: number } | null {
  const bikePt = pointOf(bikeStation.location);
  if (!bikePt) return null;
  let nearest: { station: Row; distanceKm: number } | null = null;
  for (const w of weatherStations) {
    const wPt = pointOf(w.location);
    if (!wPt) continue;
    const dist = haversineKm(bikePt[0], bikePt[1], wPt[0], wPt[1]);
    if (dist <= maxDistanceKm) {
      if (!nearest || dist < nearest.distanceKm) {
        nearest = { station: w, distanceKm: dist };
      }
    }
  }
  return nearest;
}

/** Display name for a station entity. */
export function stationName(row: Row): string {
  const name = format(row.name).trim();
  return (name.startsWith("urn:") ? "" : name) || row.id;
}

/** Total docking slots of a station. */
export function stationSlots(row: Row): number {
  const n = Number(row.totalSlotNumber);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

/** Available bikes now at a station. */
export function stationBikes(row: Row): number {
  const n = Number(row.availableBikeNumber);
  return Number.isFinite(n) && n >= 0 ? n : 0;
}

/** Free docking slots now at a station. */
export function stationFreeSlots(row: Row): number {
  const n = Number(row.freeSlotNumber);
  return Number.isFinite(n) && n >= 0 ? n : 0;
}

/** Selects the station with the most docking slots, breaking ties by name. */
export function defaultStationId(stations: Row[]): string | null {
  if (stations.length === 0) return null;
  const sorted = [...stations].sort((a, b) => {
    const slotsDiff = stationSlots(b) - stationSlots(a);
    if (slotsDiff !== 0) return slotsDiff;
    return stationName(a).localeCompare(stationName(b));
  });
  return sorted[0]?.id ?? null;
}

/** Parses station id from hash `#station?id=<urn>`. */
export function readHash(hash: string): { stationId: string | null } {
  const qIdx = hash.indexOf("?");
  if (qIdx < 0) return { stationId: null };
  const params = new URLSearchParams(hash.slice(qIdx + 1));
  const id = params.get("id");
  return { stationId: id && id.trim() !== "" ? id.trim() : null };
}

/** Formats station id into hash `#station?id=<urn>`. */
export function writeHash(stationId: string, page = "station"): string {
  return `#${page}?id=${encodeURIComponent(stationId)}`;
}

/** The ISO 8601 instant exactly `days` days before `now`. */
export function daysAgoIso(days = 7, now: Date = new Date()): string {
  return new Date(now.getTime() - days * 24 * 60 * 60 * 1000).toISOString();
}

export interface ToInputParams {
  nowSec?: number;
  totalSlots: number;
  bikePoints: TemporalPoint[];
  tempPoints?: TemporalPoint[];
  precPoints?: TemporalPoint[];
  weatherPoints?: WeatherHistoryPoint[];
}

function parseNumericCell(val: Cell): number | null {
  if (typeof val === "number" && Number.isFinite(val)) return val;
  if (typeof val === "string" && val.trim() !== "") {
    const parsed = Number(val);
    if (Number.isFinite(parsed)) return parsed;
  }
  return null;
}

/** Transforms raw SDK temporal points into the shape expected by the Rust/WASM module. */
export function toInput({
  nowSec,
  totalSlots,
  bikePoints,
  tempPoints = [],
  precPoints = [],
  weatherPoints,
}: ToInputParams): EstimateInput {
  const bikes: BikeHistoryPoint[] = bikePoints
    .map((p) => {
      const v = parseNumericCell(p.value);
      return v !== null ? { at: p.observedAt, value: v } : null;
    })
    .filter((p): p is BikeHistoryPoint => p !== null);

  let weather: WeatherHistoryPoint[] = [];
  if (weatherPoints) {
    weather = weatherPoints;
  } else {
    const map = new Map<string, { temperature: number | null; precipitation: number | null }>();
    for (const p of tempPoints) {
      const v = parseNumericCell(p.value);
      const entry = map.get(p.observedAt) ?? { temperature: null, precipitation: null };
      entry.temperature = v;
      map.set(p.observedAt, entry);
    }
    for (const p of precPoints) {
      const v = parseNumericCell(p.value);
      const entry = map.get(p.observedAt) ?? { temperature: null, precipitation: null };
      entry.precipitation = v;
      map.set(p.observedAt, entry);
    }
    weather = Array.from(map.entries())
      .map(([at, data]) => ({ at, temperature: data.temperature, precipitation: data.precipitation }))
      .sort((a, b) => a.at.localeCompare(b.at));
  }

  return {
    now: nowSec ?? Math.floor(Date.now() / 1000),
    totalSlots,
    bikes,
    weather,
  };
}
