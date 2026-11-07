/**
 * The relationships of a type, as pickers of what they may point at (DM-64, UI-84).
 *
 * A stored end is edited by picking an entity of its target class out of what the person can
 * read: the search goes through the grid's or the form's own source, so the Endpoint's Policy
 * decides what is offered exactly as it decides every other read (UI-84's security line). A
 * computed end (the inverse) is never stored and never written: it is read, one query per page,
 * as the entities whose stored end points back (DM-67).
 */
import type { EntitySource } from "./grid/source";
import { idChunks } from "./grid/source";
import type { RichCell, RichRow } from "./grid/model";

/** One end of a relationship on a type, as a grid column or a form field shows it. */
export interface RelationEnd {
  /** The class the end points at. */
  target: string;
  /** Whether it holds several targets. */
  many: boolean;
  /** Whether every entity needs a target on it; only a stored end is ever required. */
  required: boolean;
  /**
   * Set on a computed end: the stored slot of `target` that points back at this type. The column
   * is then a read-only list of links and never a picker.
   */
  inverseOf?: string;
}

/** An entity a picker offers or a computed end lists: its id, and its name where it has one. */
export interface TargetOption {
  id: string;
  name?: string;
}

/** How many entities one search offers: the person narrows by typing, never by scrolling. */
export const PICK_LIMIT = 20;

/** How many pointing entities one page of rows reads for its computed ends. */
export const INVERSE_LIMIT = 100;

type Json = Record<string, unknown>;

function object(value: unknown): Json | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Json) : undefined;
}

/**
 * The stored ends a type's generated JSON Schema states (T-2739): every property carrying
 * `x-ngsi-ld-relationship` with a `target`; `type: array` is a many end and the type's `required`
 * list the required ones. An external reference names no target and stays a text field (DM-69).
 * The computed end is not in the schema, because it is never stored; a host that has the model
 * adds it itself.
 */
export function relationsOf(type: unknown): Record<string, RelationEnd> {
  const schema = object(type);
  const properties = object(schema?.properties) ?? {};
  const required = new Set(Array.isArray(schema?.required) ? schema.required.filter((name) => typeof name === "string") : []);
  const found: Record<string, RelationEnd> = {};
  for (const [name, property] of Object.entries(properties)) {
    const target = object(object(property)?.["x-ngsi-ld-relationship"])?.target;
    if (typeof target !== "string" || target === "") {
      continue;
    }
    found[name] = { target, many: object(property)?.type === "array", required: required.has(name) };
  }
  return found;
}

/** The targets a cell names now: one object, a list of them, or several instances of the end. */
export function objectsOf(cell: RichCell | RichCell[] | undefined): string[] {
  const cells = Array.isArray(cell) ? cell : cell ? [cell] : [];
  return cells.flatMap((one) => (typeof one.object === "string" ? [one.object] : Array.isArray(one.object) ? one.object : []));
}

/** What a person reads for an entity: its `name`, else nothing and the id alone is shown. */
export function nameOf(row: RichRow): string | undefined {
  const cell = row.cells.name;
  const one = Array.isArray(cell) ? cell[0] : cell;
  if (!one) {
    return undefined;
  }
  if (one.kind === "language" && one.languageMap) {
    return Object.values(one.languageMap)[0];
  }
  return typeof one.value === "string" && one.value.trim() !== "" ? one.value : undefined;
}

/** A person's text as a regular expression that matches it literally. */
function literal(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * The entities of `target` the person can read whose id contains `text`, the first ones when
 * `text` is empty. Ids of this platform end in the entity's own local name (PF-10), which is what
 * a person types; the search asks the source, so an entity the person may not read is never
 * offered.
 */
// ponytail: matches the id only; a `name` search needs a `q` per target type and waits for a
// model that says which attribute names an entity.
export async function searchTargets(
  source: Pick<EntitySource, "query">,
  target: string,
  text: string,
  limit = PICK_LIMIT,
): Promise<TargetOption[]> {
  const typed = text.trim();
  const page = await source.query(
    { type: target, idPattern: typed === "" ? undefined : `.*${literal(typed)}.*` },
    { offset: 0, limit },
  );
  return page.rows.map((row) => ({ id: row.id, name: nameOf(row) }));
}

/** A URN as a `q` string literal. */
function quoted(urn: string): string {
  return `"${urn.replace(/[\\"]/g, (c) => `\\${c}`)}"`;
}

/**
 * The computed end of one page of rows: the entities of `target` whose stored `slot` points at
 * each of `ids`, read as one query the Endpoint's Policy narrows like any other. `full` says the
 * answer reached `limit`, so a cell may be missing some and must say so.
 */
export async function pointingAt(
  source: Pick<EntitySource, "query">,
  target: string,
  slot: string,
  ids: string[],
  limit = INVERSE_LIMIT,
): Promise<{ byId: Record<string, TargetOption[]>; full: boolean }> {
  const byId: Record<string, TargetOption[]> = {};
  if (ids.length === 0) {
    return { byId, full: false };
  }
  const page = await source.query({ type: target, q: `${slot}==${ids.map(quoted).join(",")}` }, { offset: 0, limit });
  const wanted = new Set(ids);
  for (const row of page.rows) {
    for (const id of objectsOf(row.cells[slot])) {
      if (wanted.has(id)) {
        (byId[id] ??= []).push({ id: row.id, name: nameOf(row) });
      }
    }
  }
  return { byId, full: page.rows.length >= limit };
}

/**
 * The targets of one page's relationships the person may read, by URN: one query per target type
 * (split only where the id list would outgrow a URL), never one read per cell (G-R2GRID2-017).
 * `wanted` maps each URN to the type it is read as. The source answers through the person's own
 * grants, so a target that is deleted or not readable is simply absent from the answer, and the
 * two look the same (EP-59). Only each target's name and type are kept: none of its other values
 * leaves this function.
 */
export async function resolveTargets(
  source: Pick<EntitySource, "query">,
  wanted: ReadonlyMap<string, string>,
): Promise<Map<string, { name?: string; type: string }>> {
  const byType = new Map<string, string[]>();
  for (const [urn, type] of wanted) {
    const list = byType.get(type) ?? [];
    list.push(urn);
    byType.set(type, list);
  }
  const reads = [...byType].flatMap(([type, urns]) =>
    idChunks(urns).map((ids) =>
      source.query({ type, ids }, { offset: 0, limit: ids.length, count: false }).then(
        (page) => page.rows,
        // A refused read leaves its targets unresolved: they read as ids, never as links.
        () => [] as RichRow[],
      ),
    ),
  );
  const found = new Map<string, { name?: string; type: string }>();
  for (const rows of await Promise.all(reads)) {
    for (const row of rows) {
      if (wanted.has(row.id)) found.set(row.id, { name: nameOf(row), type: row.type });
    }
  }
  return found;
}
