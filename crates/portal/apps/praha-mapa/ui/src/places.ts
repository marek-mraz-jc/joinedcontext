/**
 * Prague's places as the map reads them (T-2786): the schools, cultural venues, public toilets and
 * ticket points of the city's open data (`PointOfInterest`, told apart by `serviceCategory`) and
 * the sorting isles (`WasteContainerIsle`), all from the public space `praha-mesto`. A value the
 * entity does not carry stays `null` and is said as missing, never a zero or an empty string.
 */
import { format } from "@joinedcontext/sdk";
import type { Cell, RichCell, RichRow } from "@joinedcontext/sdk";

export const KINDS = ["school", "culture", "publicToilet", "ticketSale", "waste"] as const;
export type Kind = (typeof KINDS)[number];

/** The NGSI-LD types the map reads, and which kinds each holds. */
export const TYPES = ["PointOfInterest", "WasteContainerIsle"] as const;
export type Type = (typeof TYPES)[number];
export const KINDS_OF: Record<Type, Kind[]> = {
  PointOfInterest: ["school", "culture", "publicToilet", "ticketSale"],
  WasteContainerIsle: ["waste"],
};

/** One colour and one shape per kind: the shape is what tells them apart without colour. */
export const KIND_COLOUR: Record<Kind, string> = {
  school: "#1f5fa8",
  culture: "#9d2a6e",
  publicToilet: "#0e7490",
  ticketSale: "#b45309",
  waste: "#2f7d32",
};
export const KIND_SHAPE: Record<Kind, string> = { school: "■", culture: "◆", publicToilet: "▲", ticketSale: "●", waste: "♻" };

export interface Place {
  id: string;
  kind: Kind;
  name: string | null;
  coordinates: [number, number] | null;
  address: string | null;
  url: string | null;
  facilityType: string | null;
  openingHours: string | null;
  wheelchairAccessible: boolean | null;
  capacity: number | null;
  pupils: number | null;
  /** A sorting isle's code and who may use it. */
  stationCode: string | null;
  access: string | null;
}

function first(cell: RichCell | RichCell[] | undefined): RichCell | undefined {
  return Array.isArray(cell) ? cell[0] : cell;
}

function text(row: RichRow, attr: string): string | null {
  const value = first(row.cells[attr])?.value;
  if (typeof value === "string") return value.trim() === "" ? null : value.trim();
  if (typeof value === "number") return String(value);
  // A Smart Data Models PostalAddress (T-4329) reads as its one line.
  if (value && typeof value === "object" && !Array.isArray(value)) return format(value as Cell) || null;
  return null;
}

function number(row: RichRow, attr: string): number | null {
  const value = first(row.cells[attr])?.value;
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function flag(row: RichRow, attr: string): boolean | null {
  const value = first(row.cells[attr])?.value;
  return typeof value === "boolean" ? value : null;
}

function localized(row: RichRow, attr: string, locale: string): string | null {
  const map = first(row.cells[attr])?.languageMap;
  if (map) {
    const wanted = map[locale] ?? map.cs ?? Object.values(map)[0];
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
  if (type === "WasteContainerIsle") return "waste";
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
    url: safeUrl(text(row, "url")),
    facilityType: text(row, "facilityType"),
    openingHours: text(row, "openingHours"),
    wheelchairAccessible: flag(row, "wheelchairAccessible"),
    capacity: number(row, "capacity"),
    pupils: number(row, "pupilCount"),
    stationCode: text(row, "stationCode"),
    access: text(row, "accessRestriction"),
  };
}

/** A link the sheet may open: http or https only, so a `javascript:` URL in the data never runs. */
export function safeUrl(url: string | null): string | null {
  if (!url) return null;
  try {
    const parsed = new URL(url);
    return parsed.protocol === "https:" || parsed.protocol === "http:" ? parsed.href : null;
  } catch {
    return null;
  }
}

/** Text folded for search: lower case, without diacritics, so "skola" finds "Škola". */
export function folded(text: string): string {
  return text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/** Whether a place's name, address or facility type holds every word of the search. */
export function matches(place: Place, search: string): boolean {
  const words = folded(search).split(/\s+/).filter((word) => word !== "");
  if (words.length === 0) return true;
  const haystack = folded(`${place.name ?? ""} ${place.address ?? ""} ${place.facilityType ?? ""} ${place.stationCode ?? ""}`);
  return words.every((word) => haystack.includes(word));
}

/** By name, unnamed last. */
export function byOrder(a: Place, b: Place): number {
  if (a.name === null || b.name === null) return a.name === null ? (b.name === null ? 0 : 1) : -1;
  return a.name.localeCompare(b.name, "cs");
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
