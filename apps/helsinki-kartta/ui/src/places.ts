/**
 * Helsinki's places as the map reads them (T-2788): the libraries, health stations, swimming halls,
 * beaches and schools of the city's service register (`PointOfInterest`, told apart by
 * `serviceCategory`) and the beach water-temperature sensors (`WaterQualityObserved`), all from the
 * public space `helsinki`. A value the entity does not carry stays `null` and is said as missing.
 */
import { format } from "@joinedcontext/sdk";
import type { Cell, RichCell, RichRow } from "@joinedcontext/sdk";

export const KINDS = ["library", "healthStation", "swimmingHall", "beach", "school", "water"] as const;
export type Kind = (typeof KINDS)[number];

/** The NGSI-LD types the map reads, and which kinds each holds. */
export const TYPES = ["PointOfInterest", "WaterQualityObserved"] as const;
export type Type = (typeof TYPES)[number];
export const KINDS_OF: Record<Type, Kind[]> = {
  PointOfInterest: ["library", "healthStation", "swimmingHall", "beach", "school"],
  WaterQualityObserved: ["water"],
};

/** One colour and one shape per kind: the shape is what tells them apart without colour. */
export const KIND_COLOUR: Record<Kind, string> = {
  library: "#6941c6",
  healthStation: "#b42318",
  swimmingHall: "#0e7490",
  beach: "#b45309",
  school: "#1f5fa8",
  water: "#0369a1",
};
export const KIND_SHAPE: Record<Kind, string> = { library: "◆", healthStation: "✚", swimmingHall: "≈", beach: "▲", school: "■", water: "●" };

export interface Place {
  id: string;
  kind: Kind;
  name: string | null;
  coordinates: [number, number] | null;
  address: string | null;
  /** A water sensor's latest temperature in °C. */
  temperature: number | null;
}

function first(cell: RichCell | RichCell[] | undefined): RichCell | undefined {
  return Array.isArray(cell) ? cell[0] : cell;
}

function text(row: RichRow, attr: string): string | null {
  const value = first(row.cells[attr])?.value;
  if (typeof value === "string") return value.trim() === "" ? null : value.trim();
  if (typeof value === "number") return String(value);
  // A PostalAddress (T-3949) as its one line.
  if (value && typeof value === "object") return format(value as Cell) || null;
  return null;
}

function number(row: RichRow, attr: string): number | null {
  const value = first(row.cells[attr])?.value;
  return typeof value === "number" && Number.isFinite(value) ? value : null;
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

/** The kind of one row of `type`, or `null` for a point of interest of a category the map does not show. */
export function kindOf(row: RichRow, type: Type): Kind | null {
  if (type === "WaterQualityObserved") return "water";
  const category = text(row, "serviceCategory");
  return (KINDS_OF.PointOfInterest as string[]).includes(category ?? "") ? (category as Kind) : null;
}

export function placeOf(row: RichRow, kind: Kind, locale: string): Place {
  return {
    id: row.id,
    kind,
    name: localized(row, "name", locale),
    coordinates: point(row),
    address: text(row, "address"),
    temperature: number(row, "temperature"),
  };
}

/** Text folded for search: lower case, without diacritics, so "uimahalli" finds "Uimahalli". */
export function folded(text: string): string {
  return text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/** Whether a place's name or address holds every word of the search. */
export function matches(place: Place, search: string): boolean {
  const words = folded(search).split(/\s+/).filter((word) => word !== "");
  if (words.length === 0) return true;
  const haystack = folded(`${place.name ?? ""} ${place.address ?? ""}`);
  return words.every((word) => haystack.includes(word));
}

/** By name, unnamed last. */
export function byOrder(a: Place, b: Place): number {
  if (a.name === null || b.name === null) return a.name === null ? (b.name === null ? 0 : 1) : -1;
  return a.name.localeCompare(b.name, "fi");
}

/** The places as GeoJSON points, which is the one thing the map is given. */
export function featuresOf(places: Place[], picked: string | null) {
  return {
    type: "FeatureCollection" as const,
    features: places
      .filter((place) => place.coordinates !== null)
      .map((place) => ({
        type: "Feature" as const,
        id: place.id,
        geometry: { type: "Point" as const, coordinates: place.coordinates as [number, number] },
        properties: { id: place.id, colour: KIND_COLOUR[place.kind], picked: place.id === picked },
      })),
  };
}
