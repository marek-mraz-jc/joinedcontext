/**
 * The pipeline's `spec.lifetime` (PL-80) as the editor's Lifetime step edits it (T-4842): per
 * written type, keep the entities, end the whole entity (from a field, rolling after each write,
 * on a fixed date), or end only chosen attributes; or the `spec.expiry` sweep (PL-64). Pure, so
 * the form, the manifest and the sentence under the choice are one mapping and tested as one.
 */
import type { TFunction } from "i18next";
import i18n from "../../i18n";
import { fromIso } from "../spaces/retention";

/** The pipeline's stale-entity sweep as the manifest holds it (PL-64). */
export interface Expiry {
  after?: string;
  types?: string[];
}

/** One end time as `spec.lifetime` holds it (PL-80): `from` a slot plus a while, `rolling`, or `at`. */
export interface LifetimeMode {
  from?: string;
  plus?: string;
  notAfter?: string;
  rolling?: string;
  at?: string;
}

/** One entry of `spec.lifetime` (PL-80): the type, the whole entity's end, chosen attributes' ends. */
export interface Lifetime {
  type: string;
  entity?: LifetimeMode;
  attributes?: (LifetimeMode & { names: string[] })[];
}

/** The newest `observedAt` among a record's attributes (PL-80 `OBSERVED_AT`). */
export const OBSERVED_AT = "observedAt";

/** What happens to the entities of a type: the four choices of the step. */
export type Choice = "keep" | "entity" | "attributes" | "stale";

/** How one end is counted. */
export type EndKind = "from" | "rolling" | "at";

/** The choice an entry and the sweep are in: the entity's own end first, then attributes, then the sweep. */
export function choiceOf(entry: Lifetime | undefined, expiry: Expiry | undefined, type: string): Choice {
  if (entry?.entity) return "entity";
  if (entry?.attributes?.length) return "attributes";
  return expiry?.after && (expiry.types ?? []).includes(type) ? "stale" : "keep";
}

export function endKindOf(mode: LifetimeMode): EndKind {
  if (mode.at !== undefined) return "at";
  return mode.from !== undefined ? "from" : "rolling";
}

/** The same span counted another way: a `from` keeps `plus`, a `rolling` needs one, `at` a time. */
export function asKind(mode: LifetimeMode, kind: EndKind, now: Date): LifetimeMode {
  const span = mode.plus ?? mode.rolling ?? "PT10M";
  if (kind === "from") return { from: mode.from ?? OBSERVED_AT, plus: span, ...(mode.notAfter ? { notAfter: mode.notAfter } : {}) };
  if (kind === "rolling") return { rolling: span };
  // A day ahead, on the minute: a fixed date the person then moves.
  const at = new Date(Math.ceil(now.getTime() / 60_000) * 60_000 + 86_400_000);
  return { at: mode.at ?? at.toISOString().replace(/\.\d{3}Z$/, "Z") };
}

/** `lifetime` with the entry of `type` replaced, or removed when `entry` holds no end. */
export function withEntry(lifetime: Lifetime[] | undefined, type: string, entry: Omit<Lifetime, "type"> | undefined): Lifetime[] | undefined {
  const others = (lifetime ?? []).filter((one) => one.type !== type);
  const kept: Lifetime | undefined =
    entry && (entry.entity || entry.attributes?.length)
      ? { type, ...(entry.entity ? { entity: entry.entity } : {}), ...(entry.attributes?.length ? { attributes: entry.attributes } : {}) }
      : undefined;
  const next = kept ? [...others, kept] : others;
  return next.length > 0 ? next : undefined;
}

/** One attribute row of the step: a name and its end. */
export interface AttributeRow extends LifetimeMode {
  name: string;
}

/** The entry's attribute ends, one row per name (an entry of several names is one row each). */
export function rowsOf(entry: Lifetime | undefined): AttributeRow[] {
  return (entry?.attributes ?? []).flatMap(({ names, ...mode }) => names.map((name) => ({ ...mode, name })));
}

/** Rows back as manifest entries; a row whose attribute is not picked yet is left out. */
export function entriesOf(rows: AttributeRow[]): NonNullable<Lifetime["attributes"]> {
  return rows.filter((row) => row.name !== "").map(({ name, ...mode }) => ({ names: [name], ...mode }));
}

/** An RFC 3339 instant as the value of a `datetime-local` input, in the viewer's zone. */
export function toLocalInput(at: string | undefined): string {
  const time = at ? Date.parse(at) : Number.NaN;
  if (Number.isNaN(time)) return "";
  const local = new Date(time - new Date(time).getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 16);
}

/** A `datetime-local` value as an RFC 3339 instant in UTC, which carries its offset (PL-80). */
export function fromLocalInput(value: string): string | undefined {
  const time = Date.parse(value);
  return Number.isNaN(time) ? undefined : new Date(time).toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** Why an end cannot be saved, in words; `undefined` when it can. */
export function problemOf(mode: LifetimeMode, t: TFunction, now: Date): string | undefined {
  if (mode.at !== undefined) {
    const time = Date.parse(mode.at);
    if (Number.isNaN(time)) return t("pipelines.lifetime.problem.date");
    if (time <= now.getTime()) return t("pipelines.lifetime.problem.past");
    return undefined;
  }
  const span = mode.from !== undefined ? mode.plus : mode.rolling;
  if (mode.from === undefined && !span) return t("pipelines.lifetime.problem.span");
  return undefined;
}

/** A duration in words: "10 minutes". */
export function spanWords(iso: string | undefined, t: TFunction): string {
  const keep = iso ? fromIso(iso) : undefined;
  return keep ? t("pipelines.lifetime.span", { count: keep.amount, unit: t(`spaces.retention.unit.${keep.unit}`) }) : t("pipelines.lifetime.noSpan");
}

/** One end in words, as the end of `what` ("each Vehicle", "speed"). */
function endWords(mode: LifetimeMode, t: TFunction): string {
  if (mode.at !== undefined) {
    const time = Date.parse(mode.at);
    return t("pipelines.lifetime.says.at", { date: Number.isNaN(time) ? mode.at : new Date(time).toLocaleString(i18n.resolvedLanguage ?? i18n.language ?? "sk") });
  }
  if (mode.from === undefined) return t("pipelines.lifetime.says.rolling", { span: spanWords(mode.rolling, t) });
  const from = mode.from === OBSERVED_AT ? t("pipelines.lifetime.says.observedAt") : mode.from;
  const base = mode.plus ? t("pipelines.lifetime.says.from", { span: spanWords(mode.plus, t), from }) : t("pipelines.lifetime.says.fromExact", { from });
  return mode.notAfter ? `${base} ${t("pipelines.lifetime.says.cap", { span: spanWords(mode.notAfter, t) })}` : base;
}

/** Exactly what happens to the entities of `type`, in one or two sentences. */
export function sentence(type: string, entry: Lifetime | undefined, expiry: Expiry | undefined, t: TFunction): string {
  const choice = choiceOf(entry, expiry, type);
  if (choice === "stale") return t("pipelines.lifetime.says.stale", { type, span: spanWords(sweepIso(expiry?.after), t) });
  const parts: string[] = [];
  if (entry?.entity) {
    const renewed = entry.entity.at === undefined ? ` ${t("pipelines.lifetime.says.renewed")}` : "";
    parts.push(`${t("pipelines.lifetime.says.entity", { type, end: endWords(entry.entity, t) })}${renewed}`);
  }
  for (const row of rowsOf(entry)) parts.push(t("pipelines.lifetime.says.attribute", { type, name: row.name, end: endWords(row, t) }));
  return parts.length > 0 ? parts.join(" ") : t("pipelines.lifetime.says.keep", { type });
}

/** The sweep's `14d` / `6h` as an ISO 8601 duration. */
export function sweepIso(after: string | undefined): string | undefined {
  const match = /^([1-9][0-9]*)([hd])$/.exec(after ?? "");
  if (!match) return undefined;
  return match[2] === "d" ? `P${match[1]}D` : `PT${match[1]}H`;
}
