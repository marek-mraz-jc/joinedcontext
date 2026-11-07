/**
 * Filters generated from a DataModel (UI-33): the slots of a LinkML class become filter rows,
 * the rows compose into the NGSI-LD `q` and parse back from it. The `q` string in the manifest
 * is the only truth; the rows are a view of it, so the explorer, the pipeline studio and the
 * YAML view cannot disagree about what a pipeline reads (PL-42).
 */
import { useQuery } from "@tanstack/react-query";
import type { EnumOption, RelationEnd, ValueRule } from "@joinedcontext/sdk";
import { ApiError, readCsrfToken } from "../../api/client";
import { readModelSource } from "../../api/datamodelSource";
import { localized } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { endpointUrl } from "../endpoints/links";
import { classSlots, parseModel, relationships } from "../../pages/models/linkml";
import type { NgsiLdKind } from "../../pages/models/linkml";
import { parseResultsCount } from "../../pages/spaces/spaceFacts";

/** What the query reads: the four fields of `source.query` the manifest carries (PL-33). */
export interface EntityQuery {
  type?: string;
  attrs?: string[];
  q?: string;
  scopeQ?: string;
  /** A regular expression the ids match, as the grid's id filter asks (T-3253). */
  idPattern?: string;
}

/** One attribute a filter row may name, typed by its LinkML slot when the model declares it. */
export interface FilterSlot {
  name: string;
  range?: string;
  kind: NgsiLdKind;
  /** What a reader calls it, per language: the model's label, else its title (DM-100). */
  label?: Record<string, string>;
  /** The permissible values when the range is an enum of the model. */
  values?: string[];
  minimum?: number;
  maximum?: number;
  /** Computed by the class's formula pipeline (DM-80): shown, never edited in place. */
  formula?: boolean;
}

export const OPS = ["==", "!=", ">", ">=", "<", "<=", "~=", "!~="] as const;
export type Op = (typeof OPS)[number];

export interface Filter {
  attribute: string;
  op: Op;
  value: string;
}

const NUMERIC = ["integer", "float", "double", "decimal"];
const TEMPORAL = ["date", "datetime"];
const RESERVED = ["id", "type", "@context"];

/**
 * The LinkML text a DataModel manifest carries itself, `undefined` when it only points at the
 * file in the repository (a committed model's `spec.linkml` is a relative path).
 */
function inlineSource(model: Manifest | undefined): string | undefined {
  const source = model?.spec.linkml ?? model?.spec.source;
  return typeof source === "string" && source.includes("\n") ? source : undefined;
}

/**
 * The LinkML of a DataModel: the manifest's own when it holds it, else the file the repository
 * keeps, read through DM-56's source route and cached under the key the model editor already
 * uses. Without the fetch a committed model offers no filter rows anywhere (T-0797).
 */
export function useModelSource(project: string, model: Manifest | undefined): string | undefined {
  const inline = inlineSource(model);
  const name = model?.metadata.name;
  const loaded = useQuery({
    queryKey: ["datamodel-source", project, name],
    enabled: Boolean(name) && inline === undefined,
    // Through the typed client, as the model editor reads it under the same key (UI-07).
    queryFn: () => readModelSource(project, name ?? ""),
  });
  return inline ?? loaded.data;
}

/** The slots of one class of a LinkML model, from its text or an inline manifest; `[]` without one. */
export function filterSlotsOf(
  model: Manifest | string | undefined,
  type: string | undefined,
): FilterSlot[] {
  const source = typeof model === "string" ? model : inlineSource(model);
  if (!type || source === undefined) {
    return [];
  }
  const parsed = parseModel(source);
  const cls = parsed.classes.find((c) => c.name === type);
  // Its listed slots as the class narrows them, then its inline `attributes` (T-2720).
  const own = cls ? classSlots(parsed, cls) : [];
  const names = [...new Set([...(cls?.slots ?? []), ...own.map((slot) => slot.name)])];
  return names
    .filter((name) => !RESERVED.includes(name))
    .map((name) => {
      const slot = own.find((s) => s.name === name);
      const values = parsed.enums.find((e) => e.name === slot?.range)?.permissible_values;
      return {
        name,
        range: slot?.range,
        kind: slot?.kind ?? "Property",
        ...(slot?.label ?? slot?.title ? { label: slot?.label ?? slot?.title } : {}),
        values: values?.map((v) => v.name),
        minimum: slot?.minimum_value,
        maximum: slot?.maximum_value,
        ...(slot?.equals_expression !== undefined ? { formula: true } : {}),
      };
    });
}

/**
 * The permissible values of every enum slot of one class, by slot name, each titled in `locale`
 * where the model gives a title (UI-86): what the entity grid's `enums` takes, so such a column is
 * edited and filtered by picking. `{}` without a model or a type.
 */
export function enumsOfModel(
  model: Manifest | string | undefined,
  type: string | undefined,
  locale: string,
): Record<string, EnumOption[]> {
  const source = typeof model === "string" ? model : inlineSource(model);
  if (!type || source === undefined) {
    return {};
  }
  const parsed = parseModel(source);
  const cls = parsed.classes.find((c) => c.name === type);
  const found: Record<string, EnumOption[]> = {};
  for (const { name, range } of cls ? classSlots(parsed, cls) : []) {
    const values = parsed.enums.find((e) => e.name === range)?.permissible_values ?? [];
    if (values.length > 0) {
      found[name] = values.map((value) => ({
        value: value.name,
        title: value.title ? localized(value.title, locale, value.name) : undefined,
        description: value.description,
      }));
    }
  }
  return found;
}

/** What a LinkML range asks of a typed value, for the grid's check at the cell. */
const KIND_OF_RANGE: Record<string, ValueRule["kind"]> = {
  integer: "integer",
  float: "number",
  double: "number",
  decimal: "number",
  boolean: "boolean",
  datetime: "datetime",
  date: "date",
  uri: "uri",
  string: "string",
};

/**
 * The rule of every slot of one class that states one (T-3097): its range, bounds, pattern and
 * whether it is required, by slot name, so the grid marks a pending value that breaks it at its
 * cell. `{}` without a model or a type.
 */
export function rulesOfModel(model: Manifest | string | undefined, type: string | undefined): Record<string, ValueRule> {
  const source = typeof model === "string" ? model : inlineSource(model);
  if (!type || source === undefined) {
    return {};
  }
  const parsed = parseModel(source);
  const cls = parsed.classes.find((c) => c.name === type);
  const found: Record<string, ValueRule> = {};
  for (const slot of cls ? classSlots(parsed, cls) : []) {
    const rule: ValueRule = {};
    const kind = slot.range === undefined ? undefined : KIND_OF_RANGE[slot.range];
    if (kind !== undefined) rule.kind = kind;
    if (slot.minimum_value !== undefined) rule.minimum = slot.minimum_value;
    if (slot.maximum_value !== undefined) rule.maximum = slot.maximum_value;
    if (slot.pattern !== undefined) rule.pattern = slot.pattern;
    if (slot.required) rule.required = true;
    if (Object.keys(rule).length > 0) found[slot.name] = rule;
  }
  return found;
}

/**
 * The relationship ends of one class, by slot name (UI-84, DM-64): the stored end is edited in the
 * grid by picking entities of its target class, and the computed end, which the model derives from
 * the stored one and nothing stores, is a read-only list of what points back (DM-67). A broken
 * relationship is the model editor's diagnostic, not an end. `{}` without a model or a type.
 */
export function relationsOfModel(model: Manifest | string | undefined, type: string | undefined): Record<string, RelationEnd> {
  const source = typeof model === "string" ? model : inlineSource(model);
  if (!type || source === undefined) {
    return {};
  }
  const found: Record<string, RelationEnd> = {};
  for (const relationship of relationships(parseModel(source))) {
    const [stored, computed] =
      relationship.stored === "source" ? [relationship.source, relationship.target] : [relationship.target, relationship.source];
    if (stored.class === type) {
      found[stored.slot] = { target: computed.class, many: stored.multivalued, required: stored.required };
    }
    if (computed.class === type) {
      found[computed.slot] = { target: stored.class, many: computed.multivalued, required: false, inverseOf: stored.slot };
    }
  }
  return found;
}

/**
 * The types to offer, split by grant (T-3218, T-5622): a type the query already names stays
 * visible even where no list holds it (a YAML edit, a model still loading), so nothing a person
 * chose is silently replaced; with no grant document every type counts as granted.
 */
export function typeGroups(
  types: string[],
  current: string | undefined,
  readable: string[] | undefined,
): { granted: string[]; ungranted: string[] } {
  const offered = [...new Set([...(current && !types.includes(current) ? [current] : []), ...types])];
  const granted = readable === undefined ? offered : offered.filter((name) => readable.includes(name));
  return { granted, ungranted: offered.filter((name) => !granted.includes(name)) };
}

/** The operators a slot's range makes sense with; everything for an attribute the model lacks. */
export function opsFor(slot: FilterSlot | undefined): Op[] {
  if (!slot) {
    return [...OPS];
  }
  if (slot.range === "boolean" || slot.values || slot.kind === "Relationship") {
    return ["==", "!="];
  }
  if (NUMERIC.includes(slot.range ?? "") || TEMPORAL.includes(slot.range ?? "")) {
    return ["==", "!=", ">", ">=", "<", "<="];
  }
  return ["==", "!=", "~=", "!~="];
}

const ISO_DATE = /^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})?)?$/;

/** Whether a value goes bare into `q` (number, boolean, date) or inside quotes. */
function bare(value: string, slot: FilterSlot | undefined): boolean {
  if (slot) {
    const range = slot.range ?? "";
    return range === "boolean" || NUMERIC.includes(range) || TEMPORAL.includes(range);
  }
  return value === "true" || value === "false" || /^-?\d+(\.\d+)?$/.test(value) || ISO_DATE.test(value);
}

/** The `q` of the rows, ETSI NGSI-LD §4.9: `attr op value` joined by `;`; rows without a value are left out. */
export function qFromFilters(rows: Filter[], slots: FilterSlot[] = []): string {
  return rows
    .filter((row) => row.attribute.trim() !== "" && row.value.trim() !== "")
    .map((row) => {
      const slot = slots.find((s) => s.name === row.attribute);
      const value = row.value.trim();
      const literal = bare(value, slot) ? value : `"${value.replace(/[\\"]/g, (c) => `\\${c}`)}"`;
      return `${row.attribute.trim()}${row.op}${literal}`;
    })
    .join(";");
}

const TERM = /^([A-Za-z_][A-Za-z0-9_.]*(?:\[[^\]]+\])?)(==|!=|>=|<=|~=|!~=|>|<)(.+)$/;

/** What is wrong with a typed `q`, before the broker is asked (T-4264, T-4160 #22). */
export type QProblem = { kind: "shape" } | { kind: "unquoted"; fix: string };

const PATH = "[A-Za-z_][A-Za-z0-9_.]*(?:\\[[^\\]]+\\])?";
const LOOSE_TERM = new RegExp(`^(${PATH})(==|!=|>=|<=|~=|!~=|>|<)(.+)$`);
const BARE_TERM = new RegExp(`^!?${PATH}$`);
const NUMBER = /^-?\d+(\.\d+)?([eE][-+]?\d+)?$/;
/** A date, a time or a date-time, as the broker compares them unquoted. */
const TIME_VALUE = /^(\d{4}-\d{2}-\d{2}(T[\d:.]+(Z|[+-]\d{2}:\d{2})?)?|\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z)?)$/;

function plainValue(value: string): boolean {
  if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) return true;
  return NUMBER.test(value) || TIME_VALUE.test(value) || value === "true" || value === "false";
}

function validValue(value: string): boolean {
  if (plainValue(value)) return true;
  const range = value.split("..");
  if (range.length === 2) return range.every(plainValue);
  // A list is split outside quotes, so a comma inside "a, b" stays one value.
  const items = splitOutside(value, ",");
  return items.length > 1 && items.every(plainValue);
}

function splitOutside(text: string, separators: string): string[] {
  const parts: string[] = [];
  let current = "";
  let quoted = false;
  for (const char of text) {
    if (char === '"') quoted = !quoted;
    if (!quoted && separators.includes(char)) {
      parts.push(current);
      current = "";
    } else {
      current += char;
    }
  }
  parts.push(current);
  return parts;
}

/**
 * The first thing in `q` the broker would refuse with a parser error, said so the field can tell
 * the person how to fix it: a term that is not `attribute`, a comparison and a value, or a text
 * value without its double quotes. A `q` with brackets is left to the broker; empty is fine.
 */
export function qProblem(q: string | undefined): QProblem | undefined {
  const text = (q ?? "").trim();
  if (text === "" || /[()]/.test(text.replace(/"[^"]*"/g, '""'))) return undefined;
  for (const raw of splitOutside(text, ";|")) {
    const term = raw.trim();
    if (BARE_TERM.test(term)) continue;
    const match = LOOSE_TERM.exec(term);
    if (!match) return { kind: "shape" };
    const [, attribute, op, value] = match;
    if (!validValue(value)) {
      if (value !== value.trim()) return { kind: "shape" };
      return { kind: "unquoted", fix: `${attribute}${op}"${value.replace(/["\\]/g, "\\$&")}"` };
    }
  }
  return undefined;
}

/** The rows of a `q` in the simple grammar; `undefined` for `|`, brackets or anything else the rows cannot show. */
export function filtersFromQ(q: string | undefined): Filter[] | undefined {
  const text = (q ?? "").trim();
  if (text === "") {
    return [];
  }
  if (/[|()]/.test(text)) {
    return undefined;
  }
  const rows: Filter[] = [];
  for (const term of text.split(";")) {
    const match = TERM.exec(term.trim());
    if (!match) {
      return undefined;
    }
    const [, attribute, op, raw] = match;
    const value =
      raw.length >= 2 && raw.startsWith('"') && raw.endsWith('"')
        ? raw.slice(1, -1).replace(/\\(["\\])/g, "$1")
        : raw;
    rows.push({ attribute, op: op as Op, value });
  }
  return rows;
}

/** One entity as the gateway returns it, `keyValues` or normalized. */
export type Entity = Record<string, unknown> & { id: string };

export interface EntityPage {
  rows: Entity[];
  /** The broker's `NGSILD-Results-Count`, when it sent one. */
  count?: number;
}

function entities(body: unknown): Entity[] {
  return (Array.isArray(body) ? body : []).filter(
    (item): item is Entity =>
      typeof item === "object" && item !== null && typeof (item as { id?: unknown }).id === "string",
  );
}

async function gatewayGet(slug: string, path: string): Promise<Response> {
  // A `Request` rather than a URL string, as `api/client.ts` sends: same origin, the user's session.
  const response = await globalThis.fetch(
    new Request(endpointUrl(slug, path), { headers: { Accept: "application/ld+json" } }),
  );
  if (!response.ok) {
    throw new ApiError(response.status, response.statusText || `HTTP ${response.status}`);
  }
  return response;
}

/** One page of entities read through the endpoint with the signed-in session (UI-33). */
export async function fetchEntities(
  slug: string,
  query: EntityQuery,
  page: {
    limit: number;
    offset?: number;
    keyValues?: boolean;
    count?: boolean;
    /** `[west, south, east, north]`: only entities whose geometry lies within it (T-5061). */
    within?: [number, number, number, number];
    /** The GeoProperty `within` tests, when it is not `location`. */
    geoproperty?: string;
  },
): Promise<EntityPage> {
  if (!query.type) {
    return { rows: [] };
  }
  const params = new URLSearchParams({ type: query.type, limit: String(page.limit) });
  if (page.offset) {
    params.set("offset", String(page.offset));
  }
  if (page.keyValues !== false) {
    params.set("options", "keyValues");
  }
  if (page.count) {
    params.set("count", "true");
  }
  if (query.q?.trim()) {
    params.set("q", query.q.trim());
  }
  if (query.scopeQ?.trim()) {
    params.set("scopeQ", query.scopeQ.trim());
  }
  if (query.idPattern?.trim()) {
    params.set("idPattern", query.idPattern.trim());
  }
  if (query.attrs && query.attrs.length > 0) {
    params.set("attrs", query.attrs.join(","));
  }
  if (page.within) {
    const [w, s, e, n] = page.within;
    params.set("georel", "within");
    params.set("geometry", "Polygon");
    params.set("coordinates", JSON.stringify([[[w, s], [e, s], [e, n], [w, n], [w, s]]]));
    if (page.geoproperty && page.geoproperty !== "location") params.set("geoproperty", page.geoproperty);
  }
  const response = await gatewayGet(slug, `/ngsi-ld/v1/entities?${params.toString()}`);
  return { rows: entities(await response.json()), count: parseResultsCount(response.headers) };
}

export { MAX_REASON, reasonProblem } from "./reason";

/**
 * One entity removed through the endpoint that shows it, with the signed-in person's session
 * (UI-60) and the person's `reason`, sent percent-encoded as `JC-Reason` (DM-113). The Endpoint's
 * Policy decides: a person without the grant is refused, and the gateway's own sentence is what
 * the explorer shows.
 */
export async function deleteEntity(slug: string, id: string, reason: string): Promise<void> {
  const response = await globalThis.fetch(
    new Request(endpointUrl(slug, `/ngsi-ld/v1/entities/${encodeURIComponent(id)}`), {
      method: "DELETE",
      credentials: "same-origin",
      headers: { "x-csrf-token": readCsrfToken() ?? "", "JC-Reason": encodeURIComponent(reason.trim()) },
    }),
  );
  if (!response.ok) {
    const problem = (await response.json().catch(() => null)) as { detail?: string } | null;
    throw new ApiError(response.status, problem?.detail ?? response.statusText);
  }
}

/** One entity in full (normalized), for the detail pane. */
export async function fetchEntity(slug: string, id: string): Promise<Entity> {
  const response = await gatewayGet(slug, `/ngsi-ld/v1/entities/${encodeURIComponent(id)}`);
  const body: unknown = await response.json();
  if (typeof body !== "object" || body === null || typeof (body as { id?: unknown }).id !== "string") {
    throw new ApiError(502, "Not an entity");
  }
  return body as Entity;
}
