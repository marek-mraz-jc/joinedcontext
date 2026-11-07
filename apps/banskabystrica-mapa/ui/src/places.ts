/**
 * The city's places as the map reads them (T-2782): events the city announces, the schools of the
 * national school map and the air-quality station, all from the public space
 * `banskabystrica-verejne`. A value the entity does not carry stays `null` and is said as missing,
 * never turned into a zero or an empty string that reads like data.
 */
import { format } from "@joinedcontext/sdk";
import type { Cell, RichCell, RichRow } from "@joinedcontext/sdk";

export const KINDS = ["event", "school", "air"] as const;
export type Kind = (typeof KINDS)[number];

/** The NGSI-LD type each kind is read from. */
export const TYPE_OF: Record<Kind, string> = { event: "Event", school: "Organization", air: "AirQualityObserved" };

/** One colour and one shape per kind: the shape is what tells them apart without colour. */
export const KIND_COLOUR: Record<Kind, string> = { event: "#9d2a6e", school: "#1f5fa8", air: "#2f7d32" };
export const KIND_SHAPE: Record<Kind, string> = { event: "◆", school: "■", air: "●" };

export interface Place {
  id: string;
  kind: Kind;
  name: string | null;
  /** `[longitude, latitude]`, or `null` for a place the publisher gave no position. */
  coordinates: [number, number] | null;
  address: string | null;
  /** An event's first and last day (`YYYY-MM-DD`); the rest of an entity is the panel's to show. */
  startDate: string | null;
  endDate: string | null;
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

/** A LanguageProperty in the reader's language, else Slovak, else whichever the publisher wrote. */
function localized(row: RichRow, attr: string, locale: string): string | null {
  const cell = first(row.cells[attr]);
  const map = cell?.languageMap;
  if (map) {
    const wanted = map[locale] ?? map.sk ?? Object.values(map)[0];
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

export function placeOf(row: RichRow, kind: Kind, locale: string): Place {
  return {
    id: row.id,
    kind,
    name: localized(row, "name", locale),
    coordinates: point(row),
    address: text(row, "address"),
    startDate: text(row, "startDate"),
    endDate: text(row, "endDate"),
  };
}

/**
 * Whether an event still lies ahead or is on today: its last day, else its first, is not before
 * `today` (`YYYY-MM-DD`). An event with no date at all is kept, since nothing says it is over.
 */
export function upcoming(place: Place, today: string): boolean {
  if (place.kind !== "event") return true;
  const last = place.endDate ?? place.startDate;
  return last === null || last.slice(0, 10) >= today;
}

/** Text folded for search: lower case, without diacritics, so "skola" finds "Škola". */
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

/** Events first by their first day, then everything else by name, unnamed last. */
export function byOrder(a: Place, b: Place): number {
  if ((a.kind === "event") !== (b.kind === "event")) return a.kind === "event" ? -1 : 1;
  if (a.kind === "event" && b.kind === "event") {
    const day = (a.startDate ?? "9999").localeCompare(b.startDate ?? "9999");
    if (day !== 0) return day;
  }
  if (a.name === null || b.name === null) return a.name === null ? (b.name === null ? 0 : 1) : -1;
  return a.name.localeCompare(b.name, "sk");
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
