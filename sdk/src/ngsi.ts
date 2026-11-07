/**
 * NGSI-LD, as a dashboard needs it: read one type through the endpoint in `keyValues` form,
 * flatten what the broker returns into plain columns, and filter and aggregate rows on screen.
 *
 * The bundle reaches `/api/endpoint/{slug}/ngsi-ld/v1/entities` and nothing else (AP-49): the
 * path is built here, once, from the slug the Portal inlined.
 */
import type { Agg, Filter, Source } from "./spec";
import { DEFAULT_LIMIT, MAX_LIMIT } from "./spec";
import { queryString } from "./sdk/query";

/** A GeoJSON point or polygon as the broker serialises a GeoProperty in keyValues form. */
export interface Geo {
  type: string;
  coordinates: unknown;
}

export type Cell = string | number | boolean | Geo | null;

/**
 * A LanguageProperty as a write carries it: every language of the map, since the broker replaces
 * the attribute whole (SDK-07). A row holds one language only, so a form reads the map first.
 */
export interface LanguageMap {
  languageMap: Record<string, string>;
}

export function isLanguageMap(value: unknown): value is LanguageMap {
  return (
    typeof value === "object" &&
    value !== null &&
    "languageMap" in value &&
    typeof (value as LanguageMap).languageMap === "object" &&
    (value as LanguageMap).languageMap !== null
  );
}

/**
 * A relationship end as a write carries it: the target, or the targets of a many end (DM-64). A
 * relationship written as a plain string would reach the broker as a Property naming no target,
 * which the gateway refuses on a required end.
 */
export interface RelationshipObject {
  object: string | string[];
}

export function isRelationshipObject(value: unknown): value is RelationshipObject {
  if (typeof value !== "object" || value === null || !("object" in value)) return false;
  const object = (value as RelationshipObject).object;
  return typeof object === "string" || (Array.isArray(object) && object.every((one) => typeof one === "string"));
}

/** The targets a row's relationship cell names: one URN, or the URNs of a many end joined by ", ". */
export function targetsOf(value: Cell | undefined): string[] {
  return typeof value === "string" && value.trim() !== "" ? value.split(", ").filter((one) => one !== "") : [];
}

/** What one attribute of a write holds: a cell, every language of a map, or a relationship end. */
export type WriteValue = Cell | LanguageMap | RelationshipObject;

/** One entity with every attribute reduced to a cell. `id` and `type` are always present. */
export type Row = { id: string; type: string } & Record<string, Cell>;

export type Column = "number" | "date" | "geo" | "text";

/** The page size one request asks for; the broker's own ceiling is what caps it. */
export const PAGE = 1000;

/** The entity route of one endpoint, relative to the platform host. */
export function entitiesUrl(slug: string, source: Source, offset: number): string {
  const limit = Math.min(source.limit ?? DEFAULT_LIMIT, MAX_LIMIT);
  const params: Record<string, string | undefined> = {
    type: source.type,
    options: "keyValues",
    limit: String(Math.min(PAGE, limit - offset)),
  };
  if (offset > 0) {
    params.offset = String(offset);
  }
  // id and type come with every entity; asking for them as attributes is a 400.
  const attrs = source.attrs.filter((attr) => attr !== "id" && attr !== "type");
  if (attrs.length > 0) {
    params.attrs = attrs.join(",");
  }
  if (source.q) {
    params.q = source.q;
  }
  return `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities?${queryString(params)}`;
}

/**
 * The value a keyValues attribute stands for. The broker wraps a DateTime as `{@type, @value}`,
 * a LanguageProperty as `{lang: text}`, a GeoProperty as GeoJSON; a relationship is its URN.
 */
export function cell(value: unknown, language = "en"): Cell {
  if (value === null || value === undefined) {
    return null;
  }
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return value;
  }
  if (Array.isArray(value)) {
    return value.map((item) => String(cell(item, language) ?? "")).join(", ");
  }
  if (typeof value === "object") {
    const object = value as Record<string, unknown>;
    if ("@value" in object) {
      return cell(object["@value"], language);
    }
    // A LanguageProperty in keyValues form: picks languageMap[language], then en, then the first value.
    if (typeof object.languageMap === "object" && object.languageMap !== null) {
      const map = object.languageMap as Record<string, unknown>;
      return cell(map[language] ?? map.en ?? Object.values(map)[0], language);
    }
    if (typeof object.type === "string" && "coordinates" in object) {
      return { type: object.type, coordinates: object.coordinates };
    }
    if ("value" in object) {
      return cell(object.value, language);
    }
    if ("object" in object) {
      return cell(object.object, language);
    }
    // A PostalAddress reads as its one line, not as its first string (T-4422).
    const line = addressLine(object);
    if (line) {
      return line;
    }
    const text = Object.values(object).find((item) => typeof item === "string");
    if (typeof text === "string") {
      return text;
    }
    return JSON.stringify(object);
  }
  return String(value);
}

/** One entity as the table, the map and the charts read it. */
export function toRow(entity: Record<string, unknown>, language = "en"): Row {
  const row: Record<string, Cell> = {};
  for (const [key, value] of Object.entries(entity)) {
    if (key === "@context") {
      continue;
    }
    row[key] = cell(value, language);
  }
  return { ...row, id: String(entity.id ?? ""), type: String(entity.type ?? "") };
}

const ISO_DATE = /^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})?)?$/;

/** What kind of values a column holds, decided by the rows that carry it. */
export function columnKind(rows: Row[], attr: string): Column {
  let numbers = 0;
  let dates = 0;
  let seen = 0;
  for (const row of rows) {
    const value = row[attr];
    if (value === null || value === undefined) {
      continue;
    }
    if (typeof value === "object") {
      return "geo";
    }
    seen += 1;
    if (typeof value === "number") {
      numbers += 1;
    } else if (typeof value === "string" && ISO_DATE.test(value)) {
      dates += 1;
    }
  }
  if (seen === 0) {
    return "text";
  }
  if (numbers === seen) {
    return "number";
  }
  if (dates === seen) {
    return "date";
  }
  return "text";
}

/** The `[longitude, latitude]` of a row's geo cell, or nothing for a row without one. */
export function pointOf(value: Cell): [number, number] | null {
  if (value === null || typeof value !== "object") {
    return null;
  }
  const coordinates = value.coordinates;
  if (value.type === "Point" && Array.isArray(coordinates) && coordinates.length >= 2) {
    const [x, y] = coordinates as unknown[];
    return typeof x === "number" && typeof y === "number" ? [x, y] : null;
  }
  // A polygon or a line is placed at its first vertex: a marker, not an outline. The map draws
  // points; ponytail: outlines need a fill layer, add one when a spec asks for areas.
  let first: unknown = coordinates;
  while (Array.isArray(first) && Array.isArray(first[0])) {
    first = first[0];
  }
  if (Array.isArray(first) && typeof first[0] === "number" && typeof first[1] === "number") {
    return [first[0], first[1]];
  }
  return null;
}

/**
 * A Smart Data Models PostalAddress (`streetAddress`, `postalCode`, `addressLocality`) as the one
 * line a label shows, "Siltakatu 11, 00580 Helsinki"; "" for any other object.
 */
export function addressLine(value: Record<string, unknown>): string {
  const text = (key: string) => (typeof value[key] === "string" ? (value[key] as string).trim() : "");
  const place = [text("postalCode"), text("addressLocality")].filter(Boolean).join(" ");
  return [text("streetAddress"), place].filter(Boolean).join(", ");
}

/** A cell as text for a table or a label. */
export function format(value: Cell, kind: Column = "text"): string {
  if (value === null || value === undefined) {
    return "";
  }
  if (typeof value === "object") {
    const point = pointOf(value);
    if (point) {
      return `${point[1].toFixed(5)}, ${point[0].toFixed(5)}`;
    }
    return addressLine(value as unknown as Record<string, unknown>) || (typeof value.type === "string" ? value.type : "");
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      return "—";
    }
    return Number.isInteger(value) ? value.toLocaleString() : value.toLocaleString(undefined, { maximumFractionDigits: 2 });
  }
  if (kind === "date" && typeof value === "string") {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
  }
  return String(value);
}

/**
 * What a row is called on screen: its `name`, else its `title`, else the last segment of its URN
 * (the local id of `urn:ngsi-ld:{Type}:{domain}:{space}:{localId}`, `Helsinki-001` of
 * `urn:ngsi-ld:WeatherObserved:Helsinki-001`), never the whole URN.
 */
export function displayName(row: Row): string {
  for (const attr of ["name", "title"]) {
    const text = format(row[attr]).trim();
    if (text !== "" && !text.startsWith("urn:")) {
      return text;
    }
  }
  const id = row.id;
  return id.startsWith("urn:") ? id.slice(id.lastIndexOf(":") + 1) : id;
}

export type FilterState = Record<number, string | [number, number] | undefined>;

/** The rows that pass every filter with a value set; a blank filter passes everything. */
export function applyFilters(rows: Row[], filters: Filter[], state: FilterState, source: string): Row[] {
  return rows.filter((row) =>
    filters.every((filter, index) => {
      if ((filter.source ?? source) !== source) {
        return true;
      }
      const value = state[index];
      if (value === undefined || value === "") {
        return true;
      }
      if (filter.kind === "search") {
        const needle = String(value).toLowerCase();
        return filter.attrs.some((attr) => format(row[attr]).toLowerCase().includes(needle));
      }
      if (filter.kind === "select") {
        return format(row[filter.attr]) === value;
      }
      const number = row[filter.attr];
      if (typeof number !== "number" || !Array.isArray(value)) {
        return typeof number !== "number";
      }
      return number >= value[0] && number <= value[1];
    }),
  );
}

/** Sum, average, count and the rest over one attribute of the rows that carry a number. */
export function aggregate(rows: Row[], agg: Agg, attr?: string): number | null {
  if (agg === "count") {
    return rows.length;
  }
  const numbers = attr ? rows.map((row) => row[attr]).filter((v): v is number => typeof v === "number") : [];
  if (numbers.length === 0) {
    return null;
  }
  switch (agg) {
    case "sum":
      return numbers.reduce((a, b) => a + b, 0);
    case "avg":
      return numbers.reduce((a, b) => a + b, 0) / numbers.length;
    case "min":
      return Math.min(...numbers);
    case "max":
      return Math.max(...numbers);
    default:
      return null;
  }
}

/** The distinct values of a column, sorted, for a select filter. */
export function distinct(rows: Row[], attr: string): string[] {
  return [...new Set(rows.map((row) => format(row[attr])).filter((v) => v !== ""))].sort();
}

/** The smallest and largest number in a column, for a range filter. */
export function extent(rows: Row[], attr: string): [number, number] | null {
  const numbers = rows.map((row) => row[attr]).filter((v): v is number => typeof v === "number");
  return numbers.length === 0 ? null : [Math.min(...numbers), Math.max(...numbers)];
}

/**
 * Every entity of one source, page by page, up to its limit. A page shorter than asked ends the
 * read; a broker that answers an error ends it with that error, so a dashboard is never quietly
 * half-empty.
 */
export async function loadSource(slug: string, source: Source): Promise<Row[]> {
  const limit = Math.min(source.limit ?? DEFAULT_LIMIT, MAX_LIMIT);
  const rows: Row[] = [];
  while (rows.length < limit) {
    const response = await fetch(entitiesUrl(slug, source, rows.length), {
      headers: { Accept: "application/json" },
    });
    if (!response.ok) {
      throw new Error(`${source.type}: the endpoint answered ${response.status}`);
    }
    const page = (await response.json()) as unknown;
    if (!Array.isArray(page)) {
      throw new Error(`${source.type}: the endpoint did not answer a list`);
    }
    for (const entity of page) {
      if (typeof entity === "object" && entity !== null) {
        rows.push(toRow(entity as Record<string, unknown>));
      }
    }
    if (page.length < Math.min(PAGE, limit - (rows.length - page.length))) {
      break;
    }
  }
  return rows;
}
