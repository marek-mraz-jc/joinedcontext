/**
 * What `RecordView` shows of one value (T-5516): an NGSI-LD attribute unwrapped to what it holds,
 * a geometry as its coordinates, a scalar as text and anything else as a nested record. Pure, so
 * the rules stay testable apart from the markup.
 */
import { safeHref } from "./safeHref";

type Json = Record<string, unknown>;

/** The NGSI-LD attribute types, and the member each one keeps its value in. */
const HOLDS: Record<string, string> = {
  Property: "value",
  GeoProperty: "value",
  JsonProperty: "json",
  Relationship: "object",
  ListProperty: "valueList",
  ListRelationship: "objectList",
  LanguageProperty: "languageMap",
  VocabProperty: "vocab",
};

const GEOMETRIES = new Set(["Point", "MultiPoint", "LineString", "MultiLineString", "Polygon", "MultiPolygon"]);

export type RecordCell =
  | { kind: "text"; text: string; href?: string; unit?: string; at?: string }
  | { kind: "nested"; value: Json | unknown[]; unit?: string; at?: string };

export const isRecord = (value: unknown): value is Json =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** The value the record's copy writes: the raw form, one click away. */
export function recordJson(value: unknown): string {
  return typeof value === "string" ? value : (JSON.stringify(value, null, 2) ?? String(value));
}

/** How many positions a GeoJSON coordinates array holds. */
function positions(coordinates: unknown): number {
  if (!Array.isArray(coordinates)) return 0;
  if (typeof coordinates[0] === "number") return 1;
  return coordinates.reduce((sum: number, inner) => sum + positions(inner), 0);
}

/** A GeoJSON geometry as text: a Point's longitude and latitude, any other shape its size. */
export function geometryText(value: unknown): string | undefined {
  if (!isRecord(value) || typeof value.type !== "string" || !GEOMETRIES.has(value.type)) return undefined;
  const { type, coordinates } = value;
  if (type === "Point" && Array.isArray(coordinates) && coordinates.every((c) => typeof c === "number"))
    return `${type} ${coordinates.join(", ")}`;
  return `${type} (${positions(coordinates)})`;
}

/** One member of a record as `RecordView` shows it. */
export function cellOf(raw: unknown): RecordCell {
  let value = raw;
  let unit: string | undefined;
  let at: string | undefined;
  if (isRecord(raw) && typeof raw.type === "string" && raw.type in HOLDS && HOLDS[raw.type] in raw) {
    value = raw[HOLDS[raw.type]];
    unit = typeof raw.unitCode === "string" ? raw.unitCode : undefined;
    at = typeof raw.observedAt === "string" ? raw.observedAt : undefined;
  }
  const geometry = geometryText(value);
  if (geometry !== undefined) return { kind: "text", text: geometry, unit, at };
  if (value === null || value === undefined) return { kind: "text", text: "—", unit, at };
  if (typeof value !== "object") {
    const text = String(value);
    return { kind: "text", text, href: /^https?:\/\//.test(text) ? safeHref(text) : undefined, unit, at };
  }
  if (Array.isArray(value) && value.every((item) => item === null || typeof item !== "object"))
    return { kind: "text", text: value.map((item) => String(item ?? "—")).join(", ") || "—", unit, at };
  return { kind: "nested", value: value as Json | unknown[], unit, at };
}
