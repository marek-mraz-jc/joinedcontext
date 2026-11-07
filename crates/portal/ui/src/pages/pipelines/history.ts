/**
 * `spec.history` of a Pipeline (PL-106…PL-110, T-4845): per written attribute which history rows
 * the broker keeps and where the time of a row comes from. The form edits one attribute at a
 * time; the manifest keeps one entry per attribute name, so a person's choice reads back as made.
 * Whether the space and the broker can keep a plan is the server's record check (source
 * `history`); nothing here judges it.
 */

/** Keep every reading, keep only changes, keep the latest value only. */
export type Keep = "every" | "changes" | "latest";

export const KEEPS: readonly Keep[] = ["every", "changes", "latest"];

/** The time of a row: the broker's receive time (DM-107). */
export const RECEIVED = "received";

/** The heartbeats the form offers for keep only changes, shortest first. */
export const HEARTBEATS = ["PT15M", "PT1H", "P1D", "P7D"] as const;

export interface AttributeHistory {
  names: string[];
  keep: Keep;
  heartbeat?: string;
  observedAt?: string;
}

export interface History {
  type: string;
  attributes: AttributeHistory[];
}

/** One attribute's plan, as the form edits it. */
export interface Plan {
  keep: Keep;
  heartbeat?: string;
  observedAt?: string;
}

/** The plan of `name` of `type`, absent when the mapping decides. */
export function planOf(history: History[] | undefined, type: string, name: string): Plan | undefined {
  const entry = history?.find((one) => one.type === type);
  const found = entry?.attributes.find((one) => one.names.includes(name));
  if (!found) return undefined;
  return {
    keep: found.keep,
    ...(found.heartbeat ? { heartbeat: found.heartbeat } : {}),
    ...(found.observedAt ? { observedAt: found.observedAt } : {}),
  };
}

/** A plan the manifest accepts: a heartbeat with keep only changes and with nothing else. */
export function normalised(plan: Plan): Plan {
  return {
    keep: plan.keep,
    ...(plan.keep === "changes" ? { heartbeat: plan.heartbeat ?? "PT1H" } : {}),
    ...(plan.observedAt ? { observedAt: plan.observedAt } : {}),
  };
}

/**
 * `history` with `name` of `type` planned as `plan`, or unplanned when `plan` is absent. Other
 * names keep their plan; an entry of several names is split so only `name` moves. An empty
 * result is `undefined`, which leaves `spec.history` out of the manifest.
 */
export function withPlan(history: History[] | undefined, type: string, name: string, plan: Plan | undefined): History[] | undefined {
  const others = (history ?? []).filter((one) => one.type !== type);
  const entry = history?.find((one) => one.type === type);
  const rest: AttributeHistory[] = (entry?.attributes ?? []).flatMap((one) => {
    const names = one.names.filter((n) => n !== name);
    return names.length > 0 ? [{ ...one, names }] : [];
  });
  const attributes = plan ? [...rest, { names: [name], ...normalised(plan) }] : rest;
  attributes.sort((a, b) => (a.names[0] ?? "").localeCompare(b.names[0] ?? ""));
  const all = attributes.length > 0 ? [...others, { type, attributes }] : others;
  return all.length > 0 ? all : undefined;
}

/**
 * The date-time slots a row's time may come from, picked from the sample: those the mapped
 * records carry, or every date-time slot of the type before any record exists.
 */
export function timeSlotsOf(slots: string[], records: Record<string, unknown>[]): string[] {
  if (records.length === 0) return slots;
  return slots.filter((slot) => records.some((record) => slot in record));
}
