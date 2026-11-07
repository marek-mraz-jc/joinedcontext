/**
 * The city's places as the map reads them (T-3140): the national cultural monuments placed at
 * their buildings, the railway stations with the trains leaving them today and the air-quality
 * station, all from the public space `zilina-verejne`. A value the entity does not carry stays
 * `null` and is said as missing, never turned into a zero or an empty string that reads like data.
 */
import type { RichCell, RichRow } from "@joinedcontext/sdk";

export const KINDS = ["monument", "station", "air"] as const;
export type Kind = (typeof KINDS)[number];

/** The NGSI-LD type each kind is read from. */
export const TYPE_OF: Record<Kind, string> = { monument: "PointOfInterest", station: "GtfsStop", air: "AirQualityObserved" };

/** One colour and one shape per kind: the shape is what tells them apart without colour. */
export const KIND_COLOUR: Record<Kind, string> = { monument: "#8a4b0f", station: "#1f5fa8", air: "#2f7d32" };
export const KIND_SHAPE: Record<Kind, string> = { monument: "▲", station: "■", air: "●" };

/** The station's pollutants, in the order the sheet lists them. */
export const POLLUTANTS = ["pm10", "pm25", "no2", "o3", "co"] as const;
export type Pollutant = (typeof POLLUTANTS)[number];

/** One reading: the hour's mean, its unit and the hour it ends, each pollutant its own. */
export interface Reading {
  value: number;
  unit: string;
  at: string | null;
}

export interface Place {
  id: string;
  kind: Kind;
  name: string | null;
  /** `[longitude, latitude]`, or `null` for a place with no position (a monument with no address). */
  coordinates: [number, number] | null;
  address: string | null;
  /** A monument's register entries; ownership is the register's category only. */
  monumentNumber: string | null;
  monumentKind: string | null;
  style: string | null;
  period: string | null;
  cadastralArea: string | null;
  ownership: string | null;
  /** A station's trains leaving today. */
  departures: number | null;
  /** The air station's readings; a pollutant it did not report is absent. */
  readings: Partial<Record<Pollutant, Reading>>;
}

/** UN/CEFACT codes the pipelines write, as a person reads them. */
const UNITS: Record<string, string> = { GQ: "µg/m³", GP: "mg/m³" };

function first(cell: RichCell | RichCell[] | undefined): RichCell | undefined {
  return Array.isArray(cell) ? cell[0] : cell;
}

function text(row: RichRow, attr: string): string | null {
  const value = first(row.cells[attr])?.value;
  if (typeof value === "string") return value.trim() === "" ? null : value.trim();
  if (typeof value === "number") return String(value);
  return null;
}

function number(row: RichRow, attr: string): number | null {
  const value = first(row.cells[attr])?.value;
  return typeof value === "number" && Number.isFinite(value) ? value : null;
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

function readingOf(row: RichRow, pollutant: Pollutant): Reading | null {
  const cell = first(row.cells[pollutant]);
  const value = cell?.value;
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
  const code = typeof cell?.unitCode === "string" ? cell.unitCode : "GQ";
  const at = typeof cell?.observedAt === "string" ? cell.observedAt : text(row, "dateObserved");
  return { value, unit: UNITS[code] ?? code, at };
}

export function placeOf(row: RichRow, kind: Kind, locale: string): Place {
  const readings: Partial<Record<Pollutant, Reading>> = {};
  if (kind === "air") {
    for (const pollutant of POLLUTANTS) {
      const reading = readingOf(row, pollutant);
      if (reading) readings[pollutant] = reading;
    }
  }
  return {
    id: row.id,
    kind,
    // The register writes a monument's common name in lower case ("kaštieľ Bytčica").
    name: kind === "monument" ? capitalized(localized(row, "name", locale)) : localized(row, "name", locale),
    coordinates: point(row),
    address: text(row, "address"),
    monumentNumber: text(row, "monumentNumber"),
    monumentKind: text(row, "monumentKind"),
    style: text(row, "architecturalStyle"),
    period: text(row, "constructionPeriod"),
    cadastralArea: text(row, "cadastralArea"),
    ownership: text(row, "ownershipForm"),
    departures: number(row, "dailyDepartures"),
    readings,
  };
}

function capitalized(value: string | null): string | null {
  return value === null ? null : value.charAt(0).toLocaleUpperCase("sk") + value.slice(1);
}

/** Text folded for search: lower case, without diacritics, so "kostol" finds "KOSTOL" and "zilina" "Žilina". */
export function folded(value: string): string {
  return value.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/** Whether a place's name, address, kind of object or style holds every word of the search. */
export function matches(place: Place, search: string): boolean {
  const words = folded(search).split(/\s+/).filter((word) => word !== "");
  if (words.length === 0) return true;
  const haystack = folded([place.name, place.address, place.monumentKind, place.style, place.cadastralArea].filter(Boolean).join(" "));
  return words.every((word) => haystack.includes(word));
}

/** Stations by trains leaving, busiest first; everything else by name, unnamed last. */
export function byOrder(a: Place, b: Place): number {
  if (a.kind === "station" && b.kind === "station" && a.departures !== b.departures) {
    return (b.departures ?? -1) - (a.departures ?? -1);
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
