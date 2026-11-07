/**
 * What a pipeline's mapped records would change in its space, before anything is written
 * (T-3262): each record against the entity of the same id the space holds now. The pipeline
 * upserts, so an attribute the record does not carry is left as it is and is no change; only the
 * attributes it writes are compared.
 */
import { diffOf } from "./pipelineDebug";
import type { Change } from "./pipelineDebug";

export type Outcome = "create" | "update" | "unchanged";

export interface EntityChange {
  id: string;
  type?: string;
  outcome: Outcome;
  /** Per attribute the record writes: added to the entity or changed in it. */
  changes: Change[];
}

/** What an attribute says, without what the broker adds to it on a read. */
function comparable(attribute: unknown): unknown {
  const strip = (one: unknown) => {
    if (typeof one !== "object" || one === null || Array.isArray(one)) return one;
    const rest = { ...(one as Record<string, unknown>) };
    delete rest.createdAt;
    delete rest.modifiedAt;
    delete rest.instanceId;
    return rest;
  };
  return Array.isArray(attribute) ? attribute.map(strip) : strip(attribute);
}

/** Every record with an id, classified against the entities the space holds now, by id. */
export function changesOf(records: unknown[], current: Map<string, Record<string, unknown>>): EntityChange[] {
  const out: EntityChange[] = [];
  for (const record of records) {
    if (typeof record !== "object" || record === null || Array.isArray(record)) continue;
    const entity = record as Record<string, unknown>;
    const id = typeof entity.id === "string" ? entity.id : undefined;
    if (!id) continue;
    const type = typeof entity.type === "string" ? entity.type : undefined;
    const now = current.get(id);
    if (!now) {
      out.push({ id, type, outcome: "create", changes: [] });
      continue;
    }
    const changes = Object.entries(entity)
      .filter(([name]) => !["id", "type", "@context"].includes(name))
      // An attribute is written whole: a part of it the record leaves out is removed from it.
      .flatMap(([name, value]) => diffOf(comparable(now[name]), comparable(value), name));
    out.push({ id, type, outcome: changes.length === 0 ? "unchanged" : "update", changes });
  }
  return out;
}

/** How many of each outcome. */
export function countsOf(changes: EntityChange[]): Record<Outcome, number> {
  const counts: Record<Outcome, number> = { create: 0, update: 0, unchanged: 0 };
  for (const change of changes) counts[change.outcome] += 1;
  return counts;
}

/** The most ids one read asks the space for: the gateway's page. */
export const READ_PAGE = 100;
