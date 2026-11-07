/**
 * The grid's filter row, as an NGSI-LD query (UI-66, EP-07): one filter per column, composed into
 * the `q` and the `idPattern` the endpoint answers.
 *
 * The endpoint does the filtering, never the browser: a grid shows one page of a set that can be
 * millions, so a filter applied to the loaded rows would answer about the page and look like an
 * answer about the set. Every operator here therefore has to be expressible in ETSI NGSI-LD
 * §4.9's query language, and the value has to survive the trip: a value is data, so it is quoted
 * and escaped, and a value that goes into a pattern (`~=`, `idPattern`) has its regular-expression
 * metacharacters escaped too — otherwise `.` in a name typed by a person matches everything, and a
 * `(` answers 400.
 */
import type { MetaKey } from "./useEntityGrid";
import type { EnumOption } from "../enums";

/** What a filter asks. `between` uses both values; `empty` and `present` use none. */
export type FilterOp =
  | "contains"
  | "equals"
  | "notEquals"
  | "gt"
  | "gte"
  | "lt"
  | "lte"
  | "between"
  | "empty"
  | "present"
  | "pattern"
  /** The id begins with the text: an anchored `idPattern` (T-6328). */
  | "startsWith"
  /** One of the picked values of an enum: NGSI-LD's value list, `attr=="a","b"` (UI-86). */
  | "anyOf";

/** One column's filter: the column's own key, what it asks, and the typed value(s). */
export interface ColumnFilter {
  op: FilterOp;
  value: string;
  /** The upper bound of `between`. */
  value2?: string;
  /** The values `anyOf` picked. */
  values?: string[];
}

/**
 * What a column can be asked, by the kind of thing it holds.
 *
 * `untyped` is for a caller that has attribute names and no model to read their types from — the
 * endpoint page's condition builder is the one. It offers no operators of its own and its value
 * is judged by the shape of what was typed, which is the only rule available when the type is
 * genuinely unknown. A caller that does know the type says so, because the shape rule alone gets
 * a period or a territory code wrong.
 */
export type FilterKind =
  | "text"
  | "number"
  | "date"
  | "relationship"
  /** An attribute whose schema is an enum: its values are picked, never typed (UI-86). */
  | "enum"
  /** A true/false attribute: is yes, is no, empty or not (T-6328). */
  | "boolean"
  /** The entity's type, where the grid reads several: the picked ones narrow the `type` list. */
  | "type"
  | "id"
  | "geo"
  | "none"
  | "untyped";

const TEXT_OPS: FilterOp[] = ["contains", "equals", "notEquals", "empty", "present"];
const ORDERED_OPS: FilterOp[] = ["equals", "notEquals", "gt", "gte", "lt", "lte", "between", "empty", "present"];
const REF_OPS: FilterOp[] = ["equals", "notEquals", "empty", "present"];
const ID_OPS: FilterOp[] = ["pattern", "startsWith", "equals"];
const BOOLEAN_OPS: FilterOp[] = ["equals", "empty", "present"];
const ENUM_OPS: FilterOp[] = ["anyOf", "empty", "present"];

/** The operators a column offers; `[]` for one that cannot be filtered at the endpoint. */
export function opsForKind(kind: FilterKind): FilterOp[] {
  switch (kind) {
    case "text":
      return [...TEXT_OPS];
    case "number":
    case "date":
      return [...ORDERED_OPS];
    case "relationship":
      return [...REF_OPS];
    case "id":
      return [...ID_OPS];
    case "enum":
      return [...ENUM_OPS];
    case "boolean":
      return [...BOOLEAN_OPS];
    case "type":
      return ["anyOf"];
    // A geometry is filtered by the map's own geo query (georel), not by a typed value.
    default:
      return [];
  }
}

/** Whether a column's header filter lists its values to tick (T-6328): what `anyOf` can ask. */
/**
 * The `type` the grid asks for under a Type filter: the picked types that the grid reads, joined
 * as NGSI-LD's type list; the grid's own when none is picked. A picked name the grid does not read
 * is dropped, so the filter can only narrow.
 */
export function typeOfFilter(types: string, filter: ColumnFilter | undefined): string {
  const served = types.split(",").map((each) => each.trim()).filter(Boolean);
  const picked = filter?.op === "anyOf" ? (filter.values ?? []).filter((each) => served.includes(each)) : [];
  return picked.length > 0 ? picked.join(",") : types;
}

export function listsValues(kind: FilterKind): boolean {
  return kind === "text" || kind === "number" || kind === "date" || kind === "relationship" || kind === "enum" || kind === "boolean" || kind === "untyped";
}

/** Whether the operator needs one value, two, or none. */
export function valuesNeeded(op: FilterOp): 0 | 1 | 2 {
  if (op === "empty" || op === "present") {
    return 0;
  }
  return op === "between" ? 2 : 1;
}

/** Whether a filter is ready to be sent: its operator has the values it needs. */
export function complete(filter: ColumnFilter | undefined): boolean {
  if (!filter) {
    return false;
  }
  const needed = valuesNeeded(filter.op);
  if (needed === 0) {
    return true;
  }
  if (filter.op === "anyOf") {
    return (filter.values ?? []).length > 0;
  }
  if (filter.value.trim() === "") {
    return false;
  }
  return needed === 1 || (filter.value2 ?? "").trim() !== "";
}

const NUMBER = /^-?\d+(\.\d+)?$/;
const ISO = /^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})?)?$/;

/**
 * A value as a `q` literal: bare for a number, a boolean or a timestamp, quoted otherwise.
 *
 * `kind` is the column's, and it decides before the shape of the typed text does: a period
 * (`refPeriod == "2023"`), a territory code or any other string that happens to read as a number
 * is stored as a string, and `refPeriod==2023` asks the broker for a number and matches nothing.
 * A filter that silently answers "no rows" is worse than one that answers 400 (T-2436).
 *
 * `untyped` and an absent kind fall to the shape rule, which is what a caller with no model to
 * read has. Quoting there would turn every numeric condition an endpoint publishes into a
 * comparison against a string — `pm10>"50"` — and match nothing at all (T-2448).
 */
function literal(raw: string, kind?: FilterKind): string {
  const value = raw.trim();
  const quoted = `"${value.replace(/[\\"]/g, (c) => `\\${c}`)}"`;
  if (kind === "text" || kind === "relationship" || kind === "id") {
    return quoted;
  }
  if (value === "true" || value === "false" || NUMBER.test(value) || ISO.test(value)) {
    return value;
  }
  return quoted;
}

/** A value inside a pattern: every regular-expression metacharacter stands for itself. */
export function quotePattern(raw: string): string {
  return raw.trim().replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
}

/** A pattern as a `q` string: metacharacters stand for themselves and a quote stays inside it. */
function patternLiteral(raw: string, prefix = ""): string {
  return `"${prefix}${quotePattern(raw).replace(/"/g, '\\"')}"`;
}

/**
 * The attribute a term names: the attribute itself, or one of its metadata members, which is the
 * same dotted path the NGSI-LD query language uses for an attribute's own properties
 * (`temperature.observedAt>=…`).
 */
export function term(attr: string, meta?: MetaKey | null): string {
  return meta ? `${attr}.${meta}` : attr;
}

/** One filter as a `q` term; `undefined` when it asks for nothing. */
function termOf(
  attr: string,
  meta: MetaKey | null | undefined,
  filter: ColumnFilter,
  kind?: FilterKind,
): string | undefined {
  if (!complete(filter)) {
    return undefined;
  }
  const path = term(attr, meta);
  switch (filter.op) {
    case "empty":
      return `!${path}`;
    case "present":
      return path;
    case "contains":
      return `${path}~=${patternLiteral(filter.value)}`;
    case "equals":
      return `${path}==${literal(filter.value, kind)}`;
    case "notEquals":
      return `${path}!=${literal(filter.value, kind)}`;
    case "gt":
      return `${path}>${literal(filter.value, kind)}`;
    case "gte":
      return `${path}>=${literal(filter.value, kind)}`;
    case "lt":
      return `${path}<${literal(filter.value, kind)}`;
    case "lte":
      return `${path}<=${literal(filter.value, kind)}`;
    case "between":
      return `${path}>=${literal(filter.value, kind)};${path}<=${literal(filter.value2 ?? "", kind)}`;
    // An enum's values are strings: each is quoted, whatever it looks like. The values a header's
    // list picked keep the column's kind, so a number column asks for numbers (T-6328).
    case "anyOf":
      return `${path}==${(filter.values ?? []).map((value) => literal(value, kind === "enum" ? "text" : kind)).join(",")}`;
    // A pattern belongs to the id, which is not part of `q`.
    case "pattern":
    case "startsWith":
      return undefined;
  }
}

/** A column a filter can name: the attribute, the metadata member, and what it holds. */
export interface FilterColumn {
  key: string;
  attr: string | null;
  meta?: MetaKey | null;
  kind: FilterKind;
  /** The permissible values of an `enum` column. */
  options?: EnumOption[];
  /** The unit symbol of a number column's values, for its hint. */
  unit?: string;
  /** A LanguageProperty: `q` asks its text in any language (`name[*]`), the map itself matches nothing. */
  language?: boolean;
  /** A `date` column whose values carry a time: its filter picks a date and a time. */
  datetime?: boolean;
}

/** The attribute as a `q` path names it: a language map through `[*]`, NGSI-LD's "any language". */
function qAttr(attr: string, column: FilterColumn): string {
  return column.language && !column.meta ? `${attr}[*]` : attr;
}

/**
 * The query of a filter row: the `q` its terms compose (joined by `;`, which is NGSI-LD's "and")
 * and the `idPattern` the id column's own filter asks for. A filter that is not ready is left
 * out, so a row being typed never narrows the answer by accident.
 */
export function queryFromFilters(
  columns: FilterColumn[],
  filters: Record<string, ColumnFilter>,
): { q?: string; idPattern?: string } {
  const terms: string[] = [];
  let idPattern: string | undefined;
  for (const column of columns) {
    const filter = filters[column.key];
    if (!filter || !complete(filter)) {
      continue;
    }
    if (column.kind === "id") {
      // `idPattern` is a regular expression over the whole id, the typed text escaped so it is
      // never a pattern of the person's own: `equals` is one id, anchored at both ends; `startsWith`
      // the id's beginning, which a person types without the `urn:ngsi-ld:Type:` prefix.
      const text = quotePattern(filter.value);
      idPattern = filter.op === "equals" ? `^${text}$` : filter.op === "startsWith" ? `^(urn:ngsi-ld:[^:]+:)?${text}` : text;
      continue;
    }
    if (column.kind === "type") {
      continue;
    }
    if (!column.attr) {
      continue;
    }
    const built = termOf(qAttr(column.attr, column), column.meta, filter, column.kind);
    if (built) {
      terms.push(built);
    }
  }
  return { q: terms.length > 0 ? terms.join(";") : undefined, idPattern };
}

/** Two `q` strings as one: both must hold, which is what `;` means. */
export function andQ(...parts: (string | undefined)[]): string | undefined {
  const kept = parts.map((part) => part?.trim()).filter((part): part is string => Boolean(part));
  return kept.length > 0 ? kept.join(";") : undefined;
}

/** One condition of the filter panel: the column it asks about and what it asks. */
export interface FilterCondition {
  key: string;
  filter: ColumnFilter;
}

/**
 * The filter panel's conditions (T-4883): joined by AND (`;`) or OR (`|`), a group holding
 * conditions and groups of its own. The panel nests one level, as Baserow does; the translation
 * takes any depth.
 */
export interface FilterGroup {
  join: "and" | "or";
  conditions: (FilterCondition | FilterGroup)[];
}

function isGroup(item: FilterCondition | FilterGroup): item is FilterGroup {
  return "conditions" in item;
}

/**
 * The `q` of a filter group, `undefined` when nothing in it is ready. Every term that is itself
 * composite (a `between`, a nested group) is put in parentheses before it is joined, so an OR
 * never splits an AND it contains. The id column is left out: `idPattern` is one regular
 * expression beside `q` and cannot take part in an OR.
 */
export function queryFromGroup(columns: FilterColumn[], group: FilterGroup): string | undefined {
  const byKey = new Map(columns.map((column) => [column.key, column]));
  const parts: string[] = [];
  for (const item of group.conditions) {
    let built: string | undefined;
    if (isGroup(item)) {
      built = queryFromGroup(columns, item);
    } else {
      const column = byKey.get(item.key);
      if (column?.attr && column.kind !== "id") {
        built = termOf(qAttr(column.attr, column), column.meta, item.filter, column.kind);
      }
    }
    if (built) parts.push(built);
  }
  if (parts.length <= 1) return parts[0];
  return parts.map((part) => (/[;|]/.test(part) ? `(${part})` : part)).join(group.join === "or" ? "|" : ";");
}

/** The most text attributes one search asks: each is a regular expression the broker runs per row. */
const SEARCH_ATTRS = 8;

/**
 * The search box as a `q` (T-4883): any text attribute of the type holds the typed text,
 * case-insensitively, a LanguageProperty in any of its languages (T-5600). Number, date, place,
 * enum and metadata columns are left out: a regular expression over them matches nothing. The
 * endpoint searches the whole type with the reader's grant, never the loaded page.
 */
export function searchQuery(text: string, columns: readonly FilterColumn[]): string | undefined {
  const attrs = columns
    .filter((column) => column.kind === "text" && column.attr && !column.meta)
    .slice(0, SEARCH_ATTRS)
    .map((column) => qAttr(column.attr!, column));
  if (text.trim() === "" || attrs.length === 0) return undefined;
  const pattern = patternLiteral(text, "(?i)");
  const terms = attrs.map((attr) => `${attr}~=${pattern}`);
  return terms.length === 1 ? terms[0] : `(${terms.join("|")})`;
}
