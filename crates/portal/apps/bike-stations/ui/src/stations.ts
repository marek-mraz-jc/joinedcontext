/**
 * The city-bike docking stations as the mobility project reads them (T-3185): the helsinki
 * project's `BikeHireDockingStation`s through the project's shared space reference `city-bikes`.
 * A value the station does not carry stays `null` and is said as missing, never as zero.
 */
import type { RichCell, RichRow } from "@joinedcontext/sdk";

export const TYPE = "BikeHireDockingStation";

export interface Station {
  id: string;
  name: string | null;
  coordinates: [number, number] | null;
  bikes: number | null;
  docks: number | null;
}

/** How a station stands right now: the colour and the shape tell it without colour too. */
export type Standing = "empty" | "full" | "available" | "unknown";
export const STANDING_COLOUR: Record<Standing, string> = {
  empty: "#b42318",
  full: "#b45309",
  available: "#067647",
  unknown: "#667085",
};
export const STANDING_SHAPE: Record<Standing, string> = { empty: "○", full: "■", available: "●", unknown: "?" };

function first(cell: RichCell | RichCell[] | undefined): RichCell | undefined {
  return Array.isArray(cell) ? cell[0] : cell;
}

function text(row: RichRow, attr: string): string | null {
  const value = first(row.cells[attr])?.value;
  if (typeof value === "string") return value.trim() === "" ? null : value.trim();
  return null;
}

function count(row: RichRow, attr: string): number | null {
  const value = first(row.cells[attr])?.value;
  return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : null;
}

function localized(row: RichRow, attr: string, locale: string): string | null {
  const map = first(row.cells[attr])?.languageMap;
  if (map) {
    const wanted = map[locale] ?? map.fi ?? map.en ?? Object.values(map)[0];
    return typeof wanted === "string" && wanted.trim() !== "" ? wanted.trim() : null;
  }
  return text(row, attr);
}

function point(row: RichRow): [number, number] | null {
  const value = first(row.cells.location)?.value as { type?: unknown; coordinates?: unknown } | undefined;
  if (value?.type !== "Point" || !Array.isArray(value.coordinates)) return null;
  const [lon, lat] = value.coordinates;
  return typeof lon === "number" && typeof lat === "number" && Math.abs(lon) <= 180 && Math.abs(lat) <= 90
    ? [lon, lat]
    : null;
}

export function stationOf(row: RichRow, locale: string): Station {
  return {
    id: row.id,
    name: localized(row, "name", locale),
    coordinates: point(row),
    bikes: count(row, "availableBikeNumber"),
    docks: count(row, "freeSlotNumber"),
  };
}

/** Empty: no bike to take; full: no dock to return to; unknown when the station says neither. */
export function standingOf(station: Station): Standing {
  if (station.bikes === null && station.docks === null) return "unknown";
  if (station.bikes === 0) return "empty";
  if (station.docks === 0) return "full";
  return "available";
}

/** Text folded for search: lower case, without diacritics, so "toolo" finds "Töölö". */
export function folded(value: string): string {
  return value.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/** Whether a station's name holds every word of the search. */
export function matches(station: Station, search: string): boolean {
  const words = folded(search).split(/\s+/).filter((word) => word !== "");
  if (words.length === 0) return true;
  const name = folded(station.name ?? "");
  return words.every((word) => name.includes(word));
}

/** By name, unnamed last. */
export function byName(a: Station, b: Station): number {
  if (a.name === null || b.name === null) return a.name === null ? (b.name === null ? 0 : 1) : -1;
  return a.name.localeCompare(b.name, "fi");
}

/** The free bikes and free docks of every station that says them; a missing count adds nothing. */
export function totals(stations: Station[]): { bikes: number; docks: number } {
  return stations.reduce(
    (sum, station) => ({ bikes: sum.bikes + (station.bikes ?? 0), docks: sum.docks + (station.docks ?? 0) }),
    { bikes: 0, docks: 0 },
  );
}

/** The stations as GeoJSON points, which is the one thing the map is given. */
export function featuresOf(stations: Station[], picked: string | null) {
  return {
    type: "FeatureCollection" as const,
    features: stations
      .filter((station) => station.coordinates !== null)
      .map((station) => ({
        type: "Feature" as const,
        id: station.id,
        geometry: { type: "Point" as const, coordinates: station.coordinates as [number, number] },
        properties: { id: station.id, colour: STANDING_COLOUR[standingOf(station)], picked: station.id === picked },
      })),
  };
}
