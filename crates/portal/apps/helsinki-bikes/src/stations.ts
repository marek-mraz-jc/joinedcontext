import type { Row } from "@joinedcontext/sdk";

/** The one entity type the app's data need names (AP-04). */
export const STATION = "BikeHireDockingStation";

/** The bikes a station has now, or `null` when the station did not say. */
export function bikes(row: Row): number | null {
  const value = row.availableBikeNumber;
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

/** A station that has at least one bike to take. */
export function hasBikes(row: Row): boolean {
  return (bikes(row) ?? 0) > 0;
}

/** A station that says it has none; one that says nothing is unknown, not empty. */
export function isEmpty(row: Row): boolean {
  return bikes(row) === 0;
}

export interface Totals {
  stations: number;
  bikes: number;
  empty: number;
}

export function totals(rows: Row[]): Totals {
  return {
    stations: rows.length,
    bikes: rows.reduce((sum, row) => sum + (bikes(row) ?? 0), 0),
    empty: rows.filter(isEmpty).length,
  };
}

/** The docks a station has, or `null` when it did not say. */
function capacity(row: Row): number | null {
  const value = row.totalSlotNumber;
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : null;
}

/** The share of a station's docks holding a bike now, 0 to 1, or `null` when either count is missing. */
export function share(row: Row): number | null {
  const now = bikes(row);
  const docks = capacity(row);
  return now === null || docks === null ? null : Math.min(1, now / docks);
}

/** The attribute the map colours by: the share as a percentage, added to each row it can be computed for. */
export const SHARE = "bikeShare";

export function withShare(rows: Row[]): Row[] {
  return rows.map((row) => {
    const value = share(row);
    return value === null ? row : { ...row, [SHARE]: Math.round(value * 100) };
  });
}

/** The bands of the histogram, by bikes available now. */
export const BANDS: Array<{ label: string; min: number; max: number }> = [
  { label: "0", min: 0, max: 0 },
  { label: "1–2", min: 1, max: 2 },
  { label: "3–5", min: 3, max: 5 },
  { label: "6–10", min: 6, max: 10 },
  { label: "11–20", min: 11, max: 20 },
  { label: "21+", min: 21, max: Infinity },
];

/** How many stations fall in each band; a station that sent no count is in none. */
export function histogram(rows: Row[]): number[] {
  const counts = BANDS.map(() => 0);
  for (const row of rows) {
    const now = bikes(row);
    if (now === null) continue;
    const band = BANDS.findIndex((b) => now >= b.min && now <= b.max);
    if (band >= 0) counts[band] += 1;
  }
  return counts;
}

/**
 * The `n` fullest (highest share of docks with a bike) or emptiest stations, with their share in
 * percent; a station without both counts is left out, ties go to the one with more bikes, then by name.
 */
export function ranked(rows: Row[], n: number, order: "fullest" | "emptiest"): Array<{ name: string; percent: number; bikes: number }> {
  const known = rows.flatMap((row) => {
    const value = share(row);
    const now = bikes(row);
    return value === null || now === null ? [] : [{ name: String(row.name ?? row.id), percent: Math.round(value * 100), bikes: now }];
  });
  known.sort((a, b) =>
    order === "fullest"
      ? b.percent - a.percent || b.bikes - a.bikes || a.name.localeCompare(b.name)
      : a.percent - b.percent || a.bikes - b.bikes || a.name.localeCompare(b.name),
  );
  return known.slice(0, Math.max(0, n));
}
