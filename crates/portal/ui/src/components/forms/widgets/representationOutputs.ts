/**
 * `spec.representations.ical` and `spec.representations.feed` as the Endpoint form edits them:
 * each a list of named outputs (API/11 EP-159, owner 2026-10-22T09:30Z). The rules that refuse an
 * output (slug, duplicate name, 20 at most, a role naming a slot the model lacks, a filter the
 * broker would refuse) are jc-core's: the form's dry run answers them per member, with a 422 on
 * propose. This file holds what the editor draws and writes.
 */
import type { IcalMapping } from "./icalMapping";

export type Format = "ical" | "feed" | "widget";

/** An output's query on top of the view: it narrows, never widens (EP-157). */
export interface OutputFilter {
  q?: string;
  scopeQ?: string;
}

export interface FeedSource {
  type: string;
  preset?: string;
  mapping?: Record<string, string | string[]>;
}

export interface Output<S> {
  name: string;
  title?: Record<string, string>;
  description?: Record<string, string>;
  sources: S[];
  filter?: OutputFilter;
  [member: string]: unknown;
}

/** The catalogue widget's map defaults of one class (API/11 EP-163); an absent role keeps the widget's own. */
export interface MapSource {
  type: string;
  mapping?: { geometry?: string; colour?: string; popup?: string[] };
  cluster?: boolean;
  table?: TableStart;
}

/** Where the widget's table of one class starts (API/11 EP-164): defaults the reader can remove. */
export interface TableStart {
  filter?: string;
  orderBy?: string;
  pageLength?: number;
  columns?: string[];
}

/** The most columns a table start names (jc-core MAX_COLUMNS). */
export const MAX_COLUMNS = 30;

/** The page lengths the widget offers (DM-134). */
export const TABLE_PAGE_LENGTHS = [10, 20, 50, 100] as const;

/** A table start with one member set, or left to the model when emptied; an empty start is dropped. */
export function withTable<K extends keyof TableStart>(source: MapSource, member: K, value: TableStart[K] | undefined): MapSource {
  const table: TableStart = { ...(source.table ?? {}) };
  const empty = value === undefined || value === "" || (Array.isArray(value) && value.length === 0);
  if (empty) delete table[member];
  else table[member] = value;
  const next: MapSource = { ...source, table };
  if (Object.keys(table).length === 0) delete next.table;
  return next;
}

export type IcalOutput = Output<IcalMapping>;
export type FeedOutput = Output<FeedSource>;
export type WidgetOutput = Output<MapSource>;

export interface Outputs {
  ical: IcalOutput[];
  feed: FeedOutput[];
  widget: WidgetOutput[];
}

/** The most popup fields one class names (jc-core MAX_POPUP). */
export const MAX_POPUP = 10;

/** The GeoProperties every entity may carry besides the model's own (EP-161). */
export const CORE_GEOMETRIES = ["location", "observationSpace", "operationSpace"] as const;

/** A map role set, or left to the widget's default when emptied. */
export function withMapRole(source: MapSource, role: "geometry" | "colour", attribute: string): MapSource {
  const mapping = { ...(source.mapping ?? {}) };
  if (attribute === "") delete mapping[role];
  else mapping[role] = attribute;
  return mappedSource(source, mapping);
}

/** The popup fields with `attribute` shown or not, kept in the order `order` lists them. */
export function withPopup(source: MapSource, attribute: string, shown: boolean, order: string[]): MapSource {
  const now = new Set(source.mapping?.popup ?? []);
  if (shown) now.add(attribute);
  else now.delete(attribute);
  const popup = [...order.filter((name) => now.has(name)), ...[...now].filter((name) => !order.includes(name))];
  const mapping: NonNullable<MapSource["mapping"]> = { ...(source.mapping ?? {}), popup };
  if (popup.length === 0) delete mapping.popup;
  return mappedSource(source, mapping);
}

function mappedSource(source: MapSource, mapping: NonNullable<MapSource["mapping"]>): MapSource {
  const next: MapSource = { ...source, mapping };
  if (Object.keys(mapping).length === 0) delete next.mapping;
  return next;
}

/** The most outputs of one format an Endpoint lists (jc-core MAX_OUTPUTS). */
export const MAX_OUTPUTS = 20;

/** The name the deprecated single calendar object loads as (jc-core LEGACY_OUTPUT). */
const LEGACY_OUTPUT = "events";

export const FEED_PRESETS = ["event", "article", "generic"] as const;

/** The roles of a feed entry in table order; title and updated are required. */
export const FEED_ROLES: { role: string; required: boolean }[] = [
  { role: "title", required: true },
  { role: "updated", required: true },
  { role: "published", required: false },
  { role: "summary", required: false },
  { role: "content", required: false },
  { role: "link", required: false },
  { role: "author", required: false },
  { role: "category", required: false },
  { role: "image", required: false },
];

/** What a feed preset maps each role from, first path that has a value (jc-core preset_roles). */
const FEED_PRESET_ROLES: Record<string, Record<string, string[]>> = {
  event: {
    title: ["name"],
    updated: ["modifiedAt"],
    published: ["createdAt"],
    summary: ["description"],
    link: ["url"],
    author: ["organizer"],
    category: ["category"],
    image: ["image"],
  },
  article: {
    title: ["headline", "name"],
    updated: ["dateModified", "modifiedAt"],
    published: ["datePublished", "dateCreated"],
    summary: ["description"],
    content: ["articleBody"],
    link: ["url"],
    author: ["author"],
    category: ["category"],
    image: ["image"],
  },
  generic: { title: ["name"], updated: ["modifiedAt"], summary: ["description"], link: ["url"] },
};

export function feedPresetRoles(preset: string | undefined): Record<string, string[]> {
  return (preset && FEED_PRESET_ROLES[preset]) || {};
}

/**
 * The lists an Endpoint holds; the single calendar object of version 1 reads as one output named
 * `events`, which is what the platform loads it as and what `events.ics` keeps serving.
 */
export function outputsOf(value: unknown): Outputs {
  const block = (value ?? {}) as { ical?: unknown; feed?: unknown; widget?: unknown };
  const ical = Array.isArray(block.ical)
    ? (block.ical as IcalOutput[])
    : block.ical && typeof block.ical === "object"
      ? [{ name: LEGACY_OUTPUT, sources: [block.ical as IcalMapping] }]
      : [];
  const feed = Array.isArray(block.feed) ? (block.feed as FeedOutput[]) : [];
  const widget = Array.isArray(block.widget) ? (block.widget as WidgetOutput[]) : [];
  return { ical, feed, widget };
}

/** What the form writes back: the members it does not edit kept, an empty list left out. */
export function written(previous: unknown, next: Outputs): Record<string, unknown> | undefined {
  const rest = { ...((previous ?? {}) as Record<string, unknown>) };
  delete rest.ical;
  delete rest.feed;
  delete rest.widget;
  const out: Record<string, unknown> = { ...rest };
  if (next.ical.length > 0) out.ical = next.ical;
  if (next.feed.length > 0) out.feed = next.feed;
  if (next.widget.length > 0) out.widget = next.widget;
  return Object.keys(out).length > 0 ? out : undefined;
}

/** A slug the platform accepts: a-z, 0-9 and -, starting with a letter or digit. */
export function isSlug(name: string): boolean {
  return /^[a-z0-9][a-z0-9-]{0,39}$/.test(name);
}

/** The first `{base}`, `{base}-2`, … no output of the format holds yet. */
export function freeName(taken: string[], base: string): string {
  if (!taken.includes(base)) return base;
  let n = 2;
  while (taken.includes(`${base}-${n}`)) n += 1;
  return `${base}-${n}`;
}

/** A new output of `format` over `type`, from the preset that fits it. */
export function newOutput(format: Format, taken: string[], type: string): IcalOutput | FeedOutput | WidgetOutput {
  if (format === "widget") return { name: freeName(taken, "mapa"), sources: [{ type }] };
  if (format === "ical") {
    return { name: freeName(taken, "kalendar"), sources: [{ type, preset: "sdm-event" }] };
  }
  return { name: freeName(taken, "novinky"), sources: [{ type, preset: type === "Event" ? "event" : "generic" }] };
}

/** Names that occur more than once, which the save refuses. */
export function duplicateNames(outputs: { name: string }[]): string[] {
  const seen = new Set<string>();
  const twice = new Set<string>();
  for (const { name } of outputs) (seen.has(name) ? twice : seen).add(name);
  return [...twice];
}

/** A text per language with `lang` set to `text`, the key dropped when emptied. */
export function withText(texts: Record<string, string> | undefined, lang: string, text: string): Record<string, string> | undefined {
  const next = { ...(texts ?? {}) };
  if (text.trim() === "") delete next[lang];
  else next[lang] = text;
  return Object.keys(next).length > 0 ? next : undefined;
}

/** A feed role set to one attribute, or back to the preset's when emptied. */
export function withRole(source: FeedSource, role: string, attribute: string): FeedSource {
  const mapping = { ...(source.mapping ?? {}) };
  if (attribute === "") delete mapping[role];
  else mapping[role] = attribute;
  const next: FeedSource = { ...source, mapping };
  if (Object.keys(mapping).length === 0) delete next.mapping;
  return next;
}

/** The required roles neither the preset nor the mapping fills; the server says the rest. */
export function unmappedRoles(source: FeedSource): string[] {
  const preset = feedPresetRoles(source.preset);
  return FEED_ROLES.filter(({ required }) => required)
    .map(({ role }) => role)
    .filter((role) => source.mapping?.[role] === undefined && preset[role] === undefined);
}
