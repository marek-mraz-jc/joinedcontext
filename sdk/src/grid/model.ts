import { addressLine, toRow } from "../ngsi";
import type { Row } from "../ngsi";

export type CellKind = "property" | "relationship" | "geo" | "language" | "json" | "list" | "vocab";

export interface RichCell {
  kind: CellKind;
  value: unknown;
  unitCode?: string;
  observedAt?: string;
  datasetId?: string;
  createdAt?: string;
  modifiedAt?: string;
  object?: string | string[];
  geometryType?: string;
  languageMap?: Record<string, string>;
  sub?: Record<string, RichCell | RichCell[]>;
}

export interface RichRow {
  id: string;
  type: string;
  createdAt?: string;
  modifiedAt?: string;
  scope?: string | string[];
  cells: Record<string, RichCell | RichCell[]>;
  raw: Record<string, unknown>;
}

export const ENTITY_MEMBERS: readonly string[] = [
  "id", "type", "@context", "scope", "createdAt", "modifiedAt", "deletedAt", "observationSpace", "operationSpace",
];

const ATTR_TYPES = new Set([
  "Property", "Relationship", "GeoProperty", "LanguageProperty", "JsonProperty",
  "ListProperty", "ListRelationship", "VocabProperty", "VocabularyProperty",
]);

function detectKind(attr: Record<string, unknown>): CellKind | null {
  const type = attr.type as string | undefined;
  if (type && ATTR_TYPES.has(type)) {
    if (type === "Property") return "property";
    if (type === "Relationship") return "relationship";
    if (type === "GeoProperty") return "geo";
    if (type === "LanguageProperty") return "language";
    if (type === "JsonProperty") return "json";
    // A ListRelationship names its targets as a relationship does, so its cell reads them the same
    // way: one chip per target, each resolved and opened like any other (G-R2GRID2-018).
    if (type === "ListRelationship") return "relationship";
    if (type === "ListProperty") return "list";
    if (type === "VocabProperty" || type === "VocabularyProperty") return "vocab";
  }
  // fallback: object present -> relationship, else value present -> property
  if ("object" in attr) return "relationship";
  if ("value" in attr) return "property";
  return null;
}

function unwrapDateTime(value: unknown): unknown {
  if (typeof value === "object" && value !== null) {
    const v = value as Record<string, unknown>;
    if (v["@type"] === "DateTime" && typeof v["@value"] === "string") {
      return v["@value"];
    }
  }
  return value;
}

function collectSub(attr: Record<string, unknown>, language: string): Record<string, RichCell | RichCell[]> | undefined {
  const sub: Record<string, RichCell | RichCell[]> = {};
  let hasSub = false;
  for (const [key, val] of Object.entries(attr)) {
    if (key === "type" || key === "value" || key === "object" || key === "unitCode" || key === "observedAt" || key === "datasetId" || key === "createdAt" || key === "modifiedAt" || key === "languageMap" || key === "vocab" || key === "json" || key === "valueList" || key === "objectList") {
      continue;
    }
    if (Array.isArray(val)) {
      const items = val.map((v) => toRichCell(v, language)).filter((c): c is RichCell => c !== null);
      if (items.length > 0) {
        sub[key] = items.length === 1 ? items[0] : items;
        hasSub = true;
      }
    } else if (typeof val === "object" && val !== null) {
      const cell = toRichCell(val, language);
      if (cell) {
        sub[key] = cell;
        hasSub = true;
      }
    }
  }
  return hasSub ? sub : undefined;
}

/** NGSI-LD's null (CIM 009 4.5.9): what a merge writes to say an attribute has no value. */
export const NGSI_LD_NULL = "urn:ngsi-ld:null";

/** A ListRelationship's targets: `[{object}]` normalized, or the URNs themselves concise. */
function objectListOf(list: unknown): string[] {
  const items = Array.isArray(list) ? list : [list];
  return items.flatMap((item) => {
    const urn = typeof item === "string" ? item : (item as { object?: unknown } | null)?.object;
    return typeof urn === "string" ? [urn] : [];
  });
}

/**
 * Whether a cell holds nothing: a JSON null, NGSI-LD's null or no value at all. Such a cell reads
 * as an attribute that is not set (G-R2GRID2-020), never as the text "null".
 */
export function isEmptyCell(cell: RichCell): boolean {
  if (cell.kind === "relationship") {
    const objects = Array.isArray(cell.object) ? cell.object : cell.object ? [cell.object] : [];
    return objects.filter((urn) => urn !== NGSI_LD_NULL).length === 0;
  }
  return cell.value === null || cell.value === undefined || cell.value === NGSI_LD_NULL;
}

export function toRichCell(attr: unknown, language: string): RichCell | null {
  if (attr === null || attr === undefined) return null;
  if (typeof attr !== "object" || Array.isArray(attr)) return null;
  const a = attr as Record<string, unknown>;
  const kind = detectKind(a);
  if (!kind) return null;

  const text = (member: unknown) => (typeof member === "string" ? member : undefined);
  const cell: RichCell = {
    kind,
    value: undefined,
    observedAt: text(a.observedAt),
    datasetId: text(a.datasetId),
    createdAt: text(a.createdAt),
    modifiedAt: text(a.modifiedAt),
    sub: collectSub(a, language),
  };

  switch (kind) {
    case "property": {
      cell.value = unwrapDateTime(a.value);
      cell.unitCode = text(a.unitCode);
      break;
    }
    case "relationship": {
      const obj = a.objectList !== undefined ? objectListOf(a.objectList) : a.object;
      cell.value = obj;
      cell.object = Array.isArray(obj) ? obj.map(String) : typeof obj === "string" ? obj : undefined;
      break;
    }
    case "geo": {
      cell.value = a.value;
      cell.geometryType = (a.value as Record<string, unknown> | undefined)?.type as string | undefined;
      break;
    }
    case "language": {
      const lm = a.languageMap as Record<string, string> | undefined;
      cell.languageMap = lm;
      cell.value = lm?.[language] ?? lm?.en ?? (lm ? Object.values(lm)[0] : undefined) ?? null;
      break;
    }
    case "json": {
      cell.value = a.json;
      break;
    }
    case "list": {
      cell.value = a.valueList ?? a.objectList ?? [];
      break;
    }
    case "vocab": {
      cell.value = a.vocab;
      break;
    }
  }

  return cell;
}

export function toRichRow(entity: Record<string, unknown>, language = "en"): RichRow {
  const cells: Record<string, RichCell | RichCell[]> = {};
  for (const [key, val] of Object.entries(entity)) {
    if (ENTITY_MEMBERS.includes(key)) continue;
    // A null instance is no instance: the attribute reads as not set where none holds a value.
    const items = (Array.isArray(val) ? val : [val])
      .map((v) => toRichCell(v, language))
      .filter((c): c is RichCell => c !== null && !isEmptyCell(c));
    if (items.length > 0) {
      cells[key] = items.length === 1 ? items[0] : items;
    }
  }

  const scope = entity.scope;
  return {
    id: String(entity.id ?? ""),
    type: String(entity.type ?? ""),
    createdAt: entity.createdAt as string | undefined,
    modifiedAt: entity.modifiedAt as string | undefined,
    scope: Array.isArray(scope) ? scope.map(String) : typeof scope === "string" ? scope : undefined,
    cells,
    raw: entity,
  };
}

export function projectRow(rich: RichRow, language = "en"): Row {
  return toRow(rich.raw, language);
}

export function attributesOf(rows: RichRow[]): string[] {
  const seen = new Set<string>();
  const order: string[] = [];
  for (const row of rows) {
    for (const key of Object.keys(row.cells)) {
      if (!seen.has(key)) {
        seen.add(key);
        order.push(key);
      }
    }
  }
  return order;
}

function cellTextSingle(cell: RichCell): string {
  switch (cell.kind) {
    case "property":
      if (cell.value == null) return "";
      if (typeof cell.value === "object") {
        // A PostalAddress reads as its line (T-3949); any other object as its JSON.
        return addressLine(cell.value as Record<string, unknown>) || JSON.stringify(cell.value);
      }
      return String(cell.value);
    case "relationship":
      if (Array.isArray(cell.object)) return cell.object.join(", ");
      return cell.object ?? String(cell.value ?? "");
    case "geo":
      return cell.geometryType ?? "";
    case "language":
      return cell.value == null ? "" : String(cell.value);
    case "list":
      if (Array.isArray(cell.value)) return cell.value.map(String).join(", ");
      return String(cell.value ?? "");
    case "json":
      return cell.value === undefined ? "" : JSON.stringify(cell.value);
    case "vocab":
      return cell.value == null ? "" : String(cell.value);
  }
}

export function cellText(cell: RichCell | RichCell[] | undefined): string {
  if (cell === undefined || cell === null) return "";
  if (Array.isArray(cell)) {
    return cell.map(cellTextSingle).join(" | ");
  }
  return cellTextSingle(cell);
}
