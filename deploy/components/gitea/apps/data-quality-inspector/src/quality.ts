/**
 * Types and pure transformations for data quality inspection: converting SDK rows and
 * JSON Schema into WASM inspector input, formatting failure rules and freshness ages,
 * sorting types by validity, and serializing/deserializing hash navigation state.
 */
import type { Row } from "@joinedcontext/sdk";
import type { Lang } from "./i18n";
import { ageMessage, ruleMessage } from "./i18n";

/** The 12 entity types served in Helsinki context space on dev today. */
export const KNOWN_TYPES = [
  "AdministrativeArea",
  "AirQualityObserved",
  "Alert",
  "Article",
  "BikeHireDockingStation",
  "CityWork",
  "Event",
  "OnStreetParking",
  "PointOfInterest",
  "Vehicle",
  "WaterQualityObserved",
  "WeatherObserved",
] as const;

/** Timestamp property candidates checked for freshness, in order of preference. */
export const FRESHNESS_FIELDS = [
  "dateObserved",
  "dateModified",
  "datePublished",
  "dateIssued",
  "validFrom",
  "startDate",
] as const;

export interface TypeInspectInput {
  type: string;
  schema: unknown;
  rows: Record<string, unknown>[];
}

export interface InspectInput {
  now: number;
  types: TypeInspectInput[];
}

export interface AttributeQuality {
  name: string;
  completeness: number;
  required: boolean;
  notChecked: number;
}

export interface Finding {
  entity: string;
  attribute: string;
  rule: "type" | "enum" | "minimum" | "maximum" | "pattern" | "format" | "required" | "unknown" | string;
  detail: string;
}

export interface FreshnessQuality {
  field: string;
  medianSeconds: number;
  maxSeconds: number;
  olderThanDay: number;
}

export interface TypeQuality {
  type: string;
  entities: number;
  completeness: number;
  valid: number | null;
  attributes: AttributeQuality[];
  findings: Finding[];
  findingsTotal: number;
  freshness: FreshnessQuality | null;
}

export interface InspectOutput {
  types: TypeQuality[];
}

export interface HashState {
  type: string | null;
}

/** Resolves entity types from the published schema definitions, falling back to known types. */
export function resolveTypes(schema: Readonly<Record<string, unknown>> | null): string[] {
  if (!schema || Object.keys(schema).length === 0) {
    return [...KNOWN_TYPES];
  }
  return Object.keys(schema).sort();
}

/** Extracts the local ID part of an entity URN (after the last colon). */
export function localId(id: string): string {
  if (!id) return "";
  const idx = id.lastIndexOf(":");
  return idx >= 0 ? id.slice(idx + 1) : id;
}

/** Formats a duration in seconds into human words ("3 min", "2 h", "5 days"). */
export function formatAge(seconds: number, lang: Lang): string {
  return ageMessage(lang, seconds);
}

/** Formats a validation failure rule into human words. */
export function formatRule(rule: string, detail: string, lang: Lang): string {
  return ruleMessage(lang, rule, detail);
}

/** Formats a ratio (0..1) as a percentage string ("95 %") or "—" for null. */
export function percent(ratio: number | null | undefined, _lang: Lang): string {
  if (ratio === null || ratio === undefined || !Number.isFinite(ratio)) {
    return "—";
  }
  return `${Math.round(ratio * 100)} %`;
}

/** Converts rows and published schema into the shape expected by the WASM inspector. */
export function toInput(
  now: number,
  // The published schema as JSON: the SDK's `Schema` type names only what forms read, while the
  // document carries every keyword the validator checks (format, pattern, enum, minimum…).
  schema: Readonly<Record<string, unknown>> | null,
  rowsByType: Map<string, Row[]> | Record<string, Row[]>,
): InspectInput {
  const entries =
    rowsByType instanceof Map ? Array.from(rowsByType.entries()) : Object.entries(rowsByType);

  const types: TypeInspectInput[] = entries.map(([type, rows]) => {
    const found = schema?.[type];
    // A definition with no properties publishes nothing to check: the type has no schema.
    const typeSchema = typeof found === "object" && found !== null && "properties" in found ? found : null;
    return {
      type,
      schema: typeSchema,
      rows: rows as Record<string, unknown>[],
    };
  });

  return { now, types };
}

/** Sorts types with the worst valid % first; types with no published schema (null) sit at the end. */
export function sortTypesByQuality(types: TypeQuality[]): TypeQuality[] {
  return [...types].sort((a, b) => {
    if (a.valid !== null && b.valid !== null) {
      if (a.valid !== b.valid) return a.valid - b.valid;
      if (a.completeness !== b.completeness) return a.completeness - b.completeness;
      return a.type.localeCompare(b.type);
    }
    if (a.valid !== null) return -1;
    if (b.valid !== null) return 1;
    if (a.completeness !== b.completeness) return a.completeness - b.completeness;
    return a.type.localeCompare(b.type);
  });
}

/** Parses hash state `#quality?type=BikeHireDockingStation`. */
export function readHash(hash: string): HashState {
  if (!hash) return { type: null };
  const qIdx = hash.indexOf("?");
  const search = qIdx >= 0 ? hash.slice(qIdx + 1) : hash.startsWith("#") ? hash.slice(1) : hash;
  const params = new URLSearchParams(search);
  const type = params.get("type");
  return { type: type && type.trim() !== "" ? type.trim() : null };
}

/** Serializes selected type to hash `#quality?type=BikeHireDockingStation` or `#quality`. */
export function writeHash(type: string | null): string {
  if (!type) return "#quality";
  return `#quality?type=${encodeURIComponent(type)}`;
}

export const readView = readHash;
export const writeView = writeHash;
