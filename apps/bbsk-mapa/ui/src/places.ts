/**
 * The region's places as the map and its list read them (T-2784): the hospitals, the social
 * services and the organizations of the Banskobystrický samosprávny kraj, all from its public
 * register space `bbsk-registre`. A place opened is the SDK's entity panel's, which reads it whole;
 * this keeps what the list shows and searches. A value the entity does not carry stays `null`.
 */
import { format } from "@joinedcontext/sdk";
import type { Cell, RichCell, RichRow } from "@joinedcontext/sdk";

export const KINDS = ["hospital", "social", "organization"] as const;
export type Kind = (typeof KINDS)[number];

/** The NGSI-LD type each kind is read from. */
export const TYPE_OF: Record<Kind, string> = { hospital: "PointOfInterest", social: "PointOfInterest", organization: "PublicOrganization" };
/** Hospitals and social services share the Smart Data Model PointOfInterest; `category` says which (T-4329). */
export const CATEGORY_Q: Partial<Record<Kind, string>> = { hospital: 'category=="hospital"', social: 'category=="socialService"' };

/** One colour and one shape per kind: the shape is what tells them apart without colour. */
export const KIND_COLOUR: Record<Kind, string> = { hospital: "#b42318", social: "#6941c6", organization: "#1f5fa8" };
export const KIND_SHAPE: Record<Kind, string> = { hospital: "✚", social: "●", organization: "■" };

export interface Place {
  id: string;
  kind: Kind;
  name: string | null;
  /** `[longitude, latitude]`, or `null` for a place the publisher gave no position. */
  coordinates: [number, number] | null;
  address: string | null;
  district: string | null;
  /** A service's kind, which the search finds it by. */
  serviceKind: string | null;
}

function first(cell: RichCell | RichCell[] | undefined): RichCell | undefined {
  return Array.isArray(cell) ? cell[0] : cell;
}

function text(row: RichRow, attr: string): string | null {
  const value = first(row.cells[attr])?.value;
  if (typeof value === "string") return value.trim() === "" ? null : value.trim();
  // A Smart Data Models PostalAddress (T-4329) reads as its one line.
  if (value && typeof value === "object" && !Array.isArray(value)) return format(value as Cell) || null;
  return null;
}

/** A LanguageProperty in the reader's language, else Slovak, else whichever the publisher wrote. */
function localized(row: RichRow, attr: string, locale: string): string | null {
  const map = first(row.cells[attr])?.languageMap;
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
    district: text(row, "districtName"),
    serviceKind: text(row, "serviceKind"),
  };
}

/** Text folded for search: lower case, without diacritics, so "nemocnica" finds "Nemocnica". */
export function folded(text: string): string {
  return text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/** Whether a place's name, address or district holds every word of the search. */
export function matches(place: Place, search: string): boolean {
  const words = folded(search).split(/\s+/).filter((word) => word !== "");
  if (words.length === 0) return true;
  const haystack = folded(`${place.name ?? ""} ${place.address ?? ""} ${place.district ?? ""} ${place.serviceKind ?? ""}`);
  return words.every((word) => haystack.includes(word));
}

/** By name, unnamed last. */
export function byOrder(a: Place, b: Place): number {
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
