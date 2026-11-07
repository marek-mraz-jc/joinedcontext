/**
 * A space's retention in plain units (DM-82, DM-85, ADR-N-046): the manifest holds ISO 8601
 * durations in days, hours, minutes and seconds (`PT5M`, `PT8H`, `P30D`), the form "5 minutes",
 * "8 hours", "keep forever".
 */

export type Unit = "minutes" | "hours" | "days";
export const UNITS: Unit[] = ["minutes", "hours", "days"];

/** How long something is kept; `undefined` anywhere it is used means forever. */
export interface Keep {
  amount: number;
  unit: Unit;
}

/** One rule of `spec.retention`. */
export interface RetentionRule {
  entityType: string;
  current?: string;
  /** A duration, `off` (current state only, T-4275) or `forever` with a `reason`. */
  history?: string;
  /** Why the type keeps its whole history: only with `history: forever`. */
  reason?: string;
  /** Which rows of history the broker writes (DM-108, DM-109, DM-132); absent is `observed`. */
  record?: string;
  /** With `record: onChange`: a row at least this often (DM-109). */
  heartbeat?: string;
  deadband?: number;
  minDistance?: number;
  /** Per-attribute recordings, as `dateModified: { record: never }`; the editor keeps them as stored. */
  attributes?: Record<string, unknown>;
}

/** The `record` modes the editor offers a type, the broker's default first (DM-132). */
export const RECORDS = ["observed", "every", "onChange"] as const;
/** The `record` a rule without one gets from the broker under `ANTARES_TEMPORAL_RECORD=ruled`. */
export const RECORD_DEFAULT = "observed";
export const RECORD_ON_CHANGE = "onChange";

/**
 * `rule` recording `record` (absent: the broker's default): the settings only `onChange` reads go
 * with any other mode, so the manifest stays one the platform accepts (DM-108).
 */
export function withRecord(rule: RetentionRule, record: string | undefined, heartbeat?: string): RetentionRule {
  const { record: _record, heartbeat: _heartbeat, deadband, minDistance, ...rest } = rule;
  if (record !== RECORD_ON_CHANGE) return record === undefined ? rest : { ...rest, record };
  return {
    ...rest,
    record,
    heartbeat: heartbeat ?? rule.heartbeat ?? "PT1H",
    ...(deadband === undefined ? {} : { deadband }),
    ...(minDistance === undefined ? {} : { minDistance }),
  };
}

/** The `entityType` of the space's default rule: history of every type without its own (DM-97). */
export const ANY_TYPE = "*";

/** The history of a type that keeps its current state only (T-4275). */
export const HISTORY_OFF = "off";
/** The history of a type that keeps it all, which the manifest states with a reason. */
export const HISTORY_FOREVER = "forever";

const SECONDS: Record<Unit, number> = { minutes: 60, hours: 3600, days: 86_400 };
const LONGEST = 3650 * 86_400;
const ISO = /^P(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$/;

/** Seconds of a duration the broker reads; `undefined` for anything else, the empty `P` too. */
export function seconds(iso: string): number | undefined {
  const match = ISO.exec(iso);
  if (!match || iso === "P" || iso.endsWith("T")) return undefined;
  const [, d, h, m, s] = match.map((part) => Number(part ?? 0));
  const total = d * 86_400 + h * 3600 + m * 60 + s;
  return total >= 60 && total <= LONGEST ? total : undefined;
}

/** The manifest's duration for `keep`, or `undefined` when it is not one the broker runs. */
export function toIso(keep: Keep): string | undefined {
  if (!Number.isInteger(keep.amount) || keep.amount < 1) return undefined;
  const iso = keep.unit === "days" ? `P${keep.amount}D` : `PT${keep.amount}${keep.unit === "hours" ? "H" : "M"}`;
  return seconds(iso) === undefined ? undefined : iso;
}

/** A stored duration in the largest unit that holds it exactly; `undefined` when unreadable. */
export function fromIso(iso: string): Keep | undefined {
  const total = seconds(iso);
  if (total === undefined) return undefined;
  const unit = [...UNITS].reverse().find((one) => total % SECONDS[one] === 0) ?? "minutes";
  return { amount: Math.max(1, Math.floor(total / SECONDS[unit])), unit };
}

/** Whether `draft` keeps anything shorter than `held` does: a confirmation is owed (DM-85). */
export function shortens(held: RetentionRule[], draft: RetentionRule[]): boolean {
  const forever = Number.POSITIVE_INFINITY;
  const length = (iso?: string) =>
    iso === HISTORY_OFF ? 0 : iso === undefined ? forever : (seconds(iso) ?? forever);
  return draft.some((rule) => {
    const before = held.find((one) => one.entityType === rule.entityType);
    return (
      length(rule.current) < length(before?.current) || length(rule.history) < length(before?.history)
    );
  });
}

/**
 * The rules as the manifest writes them: a type kept forever in both, with no recording of its
 * own, has no rule. Everything else a rule holds (its recording, its attributes) is written as read.
 */
export function written(rules: RetentionRule[]): RetentionRule[] {
  return rules
    .map(({ entityType, current, history, reason, ...recording }) => ({
      entityType,
      ...(current === undefined ? {} : { current }),
      ...(history === undefined ? {} : { history }),
      ...(history === HISTORY_FOREVER && reason !== undefined ? { reason } : {}),
      ...Object.fromEntries(Object.entries(recording).filter(([, value]) => value !== undefined)),
    }))
    .filter((rule) => Object.keys(rule).length > 1);
}

/** Where an effective rule comes from, most specific first (DM-97). */
export type Layer = "entity" | "type" | "space" | "platform";

/** A rule as it holds for a type, and the layer that set it. */
export interface Effective {
  /** A duration, `off` or `forever`; `undefined` for a current state kept until deleted. */
  value?: string;
  layer: Layer | "none";
}

/**
 * The history kept for `type` (DM-97): an Entity's own `historyRetention` (a duration or `off`;
 * anything else is ignored), then the type's rule, then the space's `*` rule; without any, none.
 */
export function effectiveHistory(type: string, rules: RetentionRule[], own?: string): Effective {
  if (own !== undefined && (own === HISTORY_OFF || seconds(own) !== undefined)) return { value: own, layer: "entity" };
  const ruled = rules.find((rule) => rule.entityType === type)?.history;
  if (ruled !== undefined) return { value: ruled, layer: type === ANY_TYPE ? "space" : "type" };
  const any = rules.find((rule) => rule.entityType === ANY_TYPE)?.history;
  if (any !== undefined) return { value: any, layer: "space" };
  // The platform runs the broker with ANTARES_TEMPORAL_RECORD=ruled: no rule, no history (DM-132).
  return { value: HISTORY_OFF, layer: "platform" };
}

/**
 * How long an Entity of `type` stays without being modified (DM-83): its own `expiresAt` wins,
 * then the type's `current` rule; without either it stays until deleted.
 */
export function effectiveCurrent(type: string, rules: RetentionRule[], ownExpiresAt?: string): Effective {
  if (ownExpiresAt !== undefined) return { value: ownExpiresAt, layer: "entity" };
  const ruled = type === ANY_TYPE ? undefined : rules.find((rule) => rule.entityType === type)?.current;
  return ruled === undefined ? { layer: "none" } : { value: ruled, layer: "type" };
}

/** A space manifest's `spec.retention`, empty when it has none or something else. */
export function rulesOf(manifest: { spec?: unknown }): RetentionRule[] {
  const rules = (manifest.spec as { retention?: unknown } | undefined)?.retention;
  return Array.isArray(rules) ? (rules as RetentionRule[]) : [];
}
