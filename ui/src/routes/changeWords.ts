/**
 * A change in words (T-3274): "Jana changed Endpoint air-quality", and each field it touched as
 * a sentence of its own, "set Audience to public", read from the plan the API computes.
 */
import { humanize } from "../components/diff/PlanDiffViewer";

type T = (key: string, options?: Record<string, unknown>) => string;

/** One field the plan lists. */
export interface PlannedField {
  path: string;
  from?: unknown;
  to?: unknown;
}

/** A value short enough for a sentence: text as it is, anything else as compact JSON. */
export function shortValue(value: unknown): string {
  const text = typeof value === "string" ? value : JSON.stringify(value);
  return text.length > 60 ? `${text.slice(0, 59)}…` : text;
}

/**
 * Who did what to which resource, by the summary the change carries; a change that was not merged
 * was only proposed, and says so.
 */
export function changeSentence(
  change: {
    summary: { key: string; params?: Record<string, unknown> };
    author: { name: string };
    status: { phase: string };
  },
  t: T,
): string {
  const params = change.summary.params ?? {};
  const did = change.summary.key.split(".").pop() ?? "update";
  const merged = change.status.phase === "Merged" || change.status.phase === "Applied" || change.status.phase === "Deploying";
  return t(`approvals.history.${merged ? "said" : "proposed"}.${did}`, {
    author: change.author.name,
    kind: String(params.kind ?? ""),
    name: String(params.name ?? ""),
    defaultValue: t(change.summary.key, params),
  });
}

/** One field of the plan as a sentence. */
export function fieldSentence(field: PlannedField, t: T): string {
  const name = humanize(field.path);
  if (field.from === undefined) return t("approvals.history.field.set", { field: name, to: shortValue(field.to) });
  if (field.to === undefined) return t("approvals.history.field.removed", { field: name, from: shortValue(field.from) });
  return t("approvals.history.field.changed", { field: name, from: shortValue(field.from), to: shortValue(field.to) });
}
