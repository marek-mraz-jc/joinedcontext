/**
 * Applying edited cells through the Endpoint (T-1430; UI-67, EP-55, AG-78, UI-60).
 *
 * One code path for both places a person applies entity changes: the assistant's write card and
 * the grid's edit mode. The rule it carries is AG-78's — the browser sends each update with the
 * person's own session, so the Endpoint's Policy decides every one of them, and nothing is applied
 * on anyone's behalf. A refusal belongs to its entity and is reported in the endpoint's own words,
 * because the person has to know which value did not land and why.
 */
import type { EntitySource } from "./source";
import { SourceError } from "./source";

/** At most this many entities in one batch: `MAX_ENTITIES` in `src/agents/entity_write.rs`. */
export const MAX_ENTITIES = 50;

/** The longest reason the gateway takes in `JC-Reason` (DM-113): 1 to 500 characters. */
export const MAX_REASON = 500;

/** Whether `reason` is one the gateway takes: 1 to 500 characters once trimmed (DM-113). */
export function reasonOk(reason: string): boolean {
  const trimmed = reason.trim();
  return trimmed.length > 0 && trimmed.length <= MAX_REASON;
}

/** One attribute a person changed, as the grid and the card both hold it. */
export interface AttributeChange {
  attribute: string;
  before?: unknown;
  after: unknown;
  /** What the value is measured in, kept as it was: a number without its unit is another number. */
  unitCode?: string;
  /**
   * The kind of the attribute, so a Relationship is written as one — and so a geometry keeps its
   * own type. Writing an edited geometry as `type: "Property"` would change what the attribute is
   * at the broker, turning a GeoProperty into a Property holding an object nothing can query with
   * `georel` (UI-72, CIM 009 §4.5.4).
   */
  kind?: "property" | "relationship" | "geo";
}

export interface EntityChange {
  id: string;
  changes: AttributeChange[];
}

/** What happens to `observedAt` on a value a person corrected, chosen once per batch. */
export type Observed = "keep" | "now";

export interface Refusal {
  id: string;
  detail: string;
  /** The attribute the endpoint refused, where it said which, so the grid marks that cell. */
  slot?: string;
}

export interface ApplyResult {
  applied: string[];
  refused: Refusal[];
}

/** The NGSI-LD fragment of one entity's changes, as `PATCH …/attrs` takes it. */
export function attrsBody(
  changes: AttributeChange[],
  observed: Observed,
  now: () => string = () => new Date().toISOString(),
): Record<string, unknown> {
  const body: Record<string, unknown> = {};
  for (const change of changes) {
    const value: Record<string, unknown> =
      change.kind === "relationship"
        ? { type: "Relationship", object: change.after }
        : { type: change.kind === "geo" ? "GeoProperty" : "Property", value: change.after };
    // A geometry has no unit, and a Relationship has no value to measure.
    if (change.unitCode !== undefined && change.kind !== "relationship" && change.kind !== "geo") {
      value.unitCode = change.unitCode;
    }
    if (observed === "now") {
      // The value is being corrected now, so it was observed now: a kept `observedAt` would date
      // the new value to the old reading.
      value.observedAt = now();
    }
    body[change.attribute] = value;
  }
  return body;
}

export interface ApplyOptions {
  /**
   * Where the grid reads and writes. The write goes through the source's own `patch`, which is the
   * endpoint (or the space surface) the grid is showing: the person's session carries it, so the
   * Policy decides each update (EP-55, AG-78). A source without `patch` cannot be applied to, which
   * is how a read-only surface switches the feature off.
   */
  source: Pick<EntitySource, "patch">;
  entities: EntityChange[];
  /** Why the person writes, sent with every update as `JC-Reason` (DM-113); never empty. */
  reason: string;
  observed?: Observed;
  /** What a refusal without a problem document is called, in the caller's language. */
  fallback?: string;
  now?: () => string;
}

/**
 * Applies the changes, one `PATCH` per entity, and says which entities landed and which did not.
 *
 * One request per entity rather than a batch upsert: a batch answers 207 with a report a person
 * has to decode, and one refused entity in it looks like a refused batch. This way the grid marks
 * exactly the rows that did not land, and the ones that did are already correct.
 */
export async function applyChanges(options: ApplyOptions): Promise<ApplyResult> {
  const { source, entities, reason, observed = "keep", fallback = "the update was refused", now } = options;
  if (entities.length > MAX_ENTITIES) {
    throw new Error(`one change reaches at most ${MAX_ENTITIES} entities; this one names ${entities.length}`);
  }
  if (!source.patch) {
    throw new Error("this surface does not take writes");
  }
  if (!reasonOk(reason)) {
    throw new Error(`a write names why in 1 to ${MAX_REASON} characters`);
  }
  const applied: string[] = [];
  const refused: Refusal[] = [];
  for (const entity of entities) {
    if (entity.changes.length === 0) {
      continue;
    }
    try {
      await source.patch(entity.id, attrsBody(entity.changes, observed, now), reason.trim());
      applied.push(entity.id);
    } catch (err) {
      refused.push({ id: entity.id, detail: detailOf(err, fallback), slot: err instanceof SourceError ? err.slot : undefined });
    }
  }
  return { applied, refused };
}

/** What the person is told about one refused update: the endpoint's sentence where there is one. */
function detailOf(err: unknown, fallback: string): string {
  if (err instanceof SourceError) {
    return err.message || `${fallback} (${err.status})`;
  }
  return err instanceof Error && err.message ? err.message : fallback;
}
