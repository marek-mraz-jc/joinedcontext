/**
 * What the screen knows about a station, and how a reading becomes a colour (T-2435).
 *
 * Everything here is a pure function over what the endpoint answered, so the rules a reader is
 * shown — what counts as raised, what counts as old — are in one file and are tested without a
 * map, a browser or a network.
 */
import type { RichCell, RichRow } from "@joinedcontext/sdk";

export interface Station {
  id: string;
  /** The `{localId}` of the URN, what the station is called when it publishes no name. */
  localId: string;
  /**
   * The station's published `name`, in the reader's language (the pipelines write
   * "Stanica SK0263A, mestské pozadie"); `null` when it publishes none, and then `locales.ts`
   * names it by its id rather than inventing a name no publisher stands behind.
   */
  name: string | null;
  /** `[longitude, latitude]`, GeoJSON order; `null` when the station has no location. */
  coordinates: [number, number] | null;
  pm10: number | null;
  pm25: number | null;
  /** When the reading was taken, as the station reports it. */
  at: string | null;
}

/**
 * How a station reads right now. `stale` is a band of its own and is checked before the
 * thresholds: a station that stopped reporting at 30 µg/m³ would otherwise be drawn in the same
 * colour as clean air, which is the one thing this screen must not say.
 */
export type Band = "low" | "raised" | "high" | "stale" | "unknown";

/** Directive 2008/50/EC as `docs/Development/11` §2 quotes it, in µg/m³. */
export const PM10_RAISED = 35;
export const PM10_HIGH = 50;

/** A reading older than this answers about the past, not about the air now. */
export const STALE_AFTER_HOURS = 3;

/** The colour of each band on the map. The list beside it repeats the band in words (UI-30). */
export const BAND_COLOUR: Record<Band, string> = {
  low: "#1a7f37",
  raised: "#bf8700",
  high: "#b42318",
  stale: "#6e7781",
  unknown: "#6e7781",
};

function first(cell: RichCell | RichCell[] | undefined): RichCell | undefined {
  return Array.isArray(cell) ? cell[0] : cell;
}

function numberOf(cell: RichCell | RichCell[] | undefined): number | null {
  const value = first(cell)?.value;
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function textOf(cell: RichCell | RichCell[] | undefined): string | null {
  const value = first(cell)?.value;
  return typeof value === "string" && value !== "" ? value : null;
}

/** `[longitude, latitude]` of a GeoJSON Point, or `null` for anything else. */
export function pointOf(cell: RichCell | RichCell[] | undefined): [number, number] | null {
  const value = first(cell)?.value as { type?: unknown; coordinates?: unknown } | undefined;
  if (!value || value.type !== "Point" || !Array.isArray(value.coordinates)) return null;
  const [longitude, latitude] = value.coordinates;
  if (typeof longitude !== "number" || typeof latitude !== "number") return null;
  if (!Number.isFinite(longitude) || !Number.isFinite(latitude)) return null;
  return [longitude, latitude];
}

/** The `{localId}` of `urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}`, or the whole id. */
export function localIdOf(id: string): string {
  const segments = id.split(":");
  return segments.length > 5 ? segments.slice(5).join(":") : id;
}

export function toStation(row: RichRow): Station {
  const pm10 = row.cells.pm10;
  return {
    id: row.id,
    localId: localIdOf(row.id),
    name: textOf(row.cells.name),
    coordinates: pointOf(row.cells.location),
    pm10: numberOf(pm10),
    pm25: numberOf(row.cells.pm25),
    // `dateObserved` is the station's own word for when it measured; `observedAt` on the value is
    // the same instant when the writer set it, and is the fallback rather than "now".
    at: textOf(row.cells.dateObserved) ?? first(pm10)?.observedAt ?? textOf(row.cells.observedAt),
  };
}

/** Milliseconds since `iso`, or `null` when there is no readable timestamp. */
export function ageOf(at: string | null, now: Date): number | null {
  if (!at) return null;
  const taken = new Date(at).getTime();
  if (Number.isNaN(taken)) return null;
  return now.getTime() - taken;
}

export function bandOf(station: Station, now: Date): Band {
  const age = ageOf(station.at, now);
  if (age === null || age > STALE_AFTER_HOURS * 3_600_000) return "stale";
  if (station.pm10 === null) return "unknown";
  if (station.pm10 >= PM10_HIGH) return "high";
  if (station.pm10 >= PM10_RAISED) return "raised";
  return "low";
}

/** The stations of an answer, the freshest reading first, so the list opens on what is current. */
export function stationsOf(rows: RichRow[], now: Date): Station[] {
  return rows
    .map(toStation)
    .sort((left, right) => (ageOf(left.at, now) ?? Infinity) - (ageOf(right.at, now) ?? Infinity));
}
