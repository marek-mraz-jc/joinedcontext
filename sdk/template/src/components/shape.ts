import { columnKind, distinct } from "@joinedcontext/sdk";
import type { FilterDef, GridColumn, Row, TypeSchema } from "@joinedcontext/sdk";

/** What a page needs to know about one entity type to pick its filters, map and charts. */
export interface Shape {
  attrs: string[];
  geo?: string;
  label?: string;
  numbers: string[];
  time?: string;
  categories: string[];
  texts: string[];
}

type Property = Record<string, unknown>;

function types(property: Property): string[] {
  const declared = property.type;
  return Array.isArray(declared) ? declared.map(String) : declared === undefined ? [] : [String(declared)];
}

/**
 * Reads the endpoint's JSON Schema for the type; where it describes nothing (a model not
 * compiled yet), the rows decide. Model Tools marks each property with `x-ngsi-ld-kind`.
 */
export function shapeOf(schema: TypeSchema | null | undefined, rows: Row[]): Shape {
  const properties = (schema?.properties ?? {}) as Record<string, Property>;
  const declared = Object.keys(properties).filter((name) => name !== "id" && name !== "type");
  const seen = [...new Set(rows.slice(0, 50).flatMap((row) => Object.keys(row)))].filter(
    (name) => name !== "id" && name !== "type" && name !== "@context",
  );
  const attrs = declared.length > 0 ? declared : seen;

  const shape: Shape = { attrs, numbers: [], categories: [], texts: [] };
  for (const name of attrs) {
    const property = properties[name];
    const kind = property ? property["x-ngsi-ld-kind"] : undefined;
    const jsonTypes = property ? types(property) : [];
    const column = columnKind(rows, name);
    if (kind === "GeoProperty" || (!property && column === "geo") || (property && jsonTypes.includes("object") && name === "location")) {
      shape.geo ??= name;
    } else if (kind === "Relationship") {
      continue;
    } else if (jsonTypes.includes("number") || jsonTypes.includes("integer") || (!property && column === "number")) {
      shape.numbers.push(name);
    } else if (property?.format === "date-time" || property?.format === "date" || (!property && column === "date")) {
      shape.time ??= name;
    } else if (Array.isArray(property?.enum)) {
      shape.categories.push(name);
    } else if (jsonTypes.includes("string") || jsonTypes.includes("object") || (!property && column === "text")) {
      const values = distinct(rows, name).length;
      if (rows.length > 0 && values >= 2 && values <= 30 && values < rows.length) {
        shape.categories.push(name);
      } else {
        shape.texts.push(name);
      }
    }
  }
  shape.label = [...shape.texts, ...shape.categories].find((name) => /name|title|label/i.test(name)) ?? shape.texts[0];
  return shape;
}

/**
 * What a row shows in a table, most telling first: its name, measures, categories, time. A map
 * shows the geometry and an entity panel the relationships, so neither takes a column.
 */
export function columnsOf(shape: Shape): GridColumn[] {
  const names = [shape.label, ...shape.numbers.slice(0, 3), ...shape.categories.slice(0, 2), shape.time];
  return [...new Set(names)].filter((attr) => attr !== undefined).map((attr) => ({ attr, label: spokenName(attr) }));
}

/** `stationName` in a sentence: `station name`. */
export function wordsOf(attr: string): string {
  return attr.replace(/([a-z\d])([A-Z])/g, "$1 $2").toLowerCase();
}

/** `stationName` as a header reads it: `Station name`; `pm25` stays. */
export function spokenName(attr: string): string {
  const words = wordsOf(attr);
  return /\d/.test(attr) ? words : words.charAt(0).toUpperCase() + words.slice(1);
}

/** Search over the texts, a select per category (two at most), a range and a date range. */
export function filtersOf(shape: Shape): FilterDef[] {
  const filters: FilterDef[] = [];
  if (shape.texts.length > 0) filters.push({ kind: "search", attrs: shape.texts });
  for (const attr of shape.categories.slice(0, 2)) filters.push({ kind: "select", attr, label: spokenName(attr) });
  if (shape.numbers[0]) filters.push({ kind: "range", attr: shape.numbers[0], label: spokenName(shape.numbers[0]) });
  if (shape.time) filters.push({ kind: "dateRange", attr: shape.time, label: spokenName(shape.time) });
  return filters;
}
