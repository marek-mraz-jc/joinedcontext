import { format, pointOf } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";

/** The one entity type the app's data need names (AP-04). */
export const EVENT = "Event";

/** Every day and time the page shows is Helsinki's, whatever the reader's clock says. */
export const ZONE = "Europe/Helsinki";

/** The query of the events that have not ended before `from` (NGSI-LD q, a DateTime is unquoted). */
export function upcomingQuery(from: Date): string {
  return `endDate>=${from.toISOString().replace(/\.\d{3}Z$/, "Z")}`;
}

/** A date attribute as a Date, or `null` when it is missing or not a date. */
export function dateOf(row: Row, attr: "startDate" | "endDate"): Date | null {
  const raw = row[attr];
  if (typeof raw !== "string") return null;
  const parsed = new Date(raw);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

/** A text attribute as the row holds it, in the reader's language (the SDK resolves a language map). */
export function textOf(row: Row, attr: string): string {
  return format(row[attr]).trim();
}

/** The event's link to its source, only when it is an `https:` address. */
export function sourceOf(row: Row): string {
  const source = textOf(row, "source");
  return /^https:\/\/[^\s]+$/.test(source) ? source : "";
}

/** The Linked Events registers the city's feed carries, by the prefix of the event's id. */
const REGISTERS: Record<string, string> = {
  helsinki: "City of Helsinki",
  espoo_le: "City of Espoo",
  kulke: "Culture centres",
  hkm: "Helsinki City Museum",
  elo: "Economic Development Division",
};

/**
 * Who publishes the event: the Linked Events register named by the prefix of its local id
 * (`helsinki-agf…`, `espoo_le-agn…`). An Event carries no category, and the register is the one
 * grouping its id holds; an id with no prefix is "Other".
 */
export function registerOf(row: Row): string {
  const local = row.id.slice(row.id.lastIndexOf(":") + 1);
  const dash = local.indexOf("-");
  if (dash <= 0) return "Other";
  const prefix = local.slice(0, dash);
  return REGISTERS[prefix] ?? prefix;
}

const fold = (value: string) => value.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

export interface Filter {
  from?: Date | null;
  to?: Date | null;
  query?: string;
  /** One day, as `YYYY-MM-DD` in Helsinki. */
  day?: string | null;
  register?: string | null;
}

/**
 * The events a reader asked for, soonest first: overlapping [from, to] (an open side is no limit,
 * an event with no dates is kept), holding every word of `query` in its name, place or
 * description ignoring case and accents, taking place on `day` and published by `register` when set.
 */
export function filterEvents(rows: Row[], { from = null, to = null, query = "", day = null, register = null }: Filter = {}): Row[] {
  const words = fold(query).split(/\s+/).filter(Boolean);
  const start = (row: Row) => dateOf(row, "startDate");
  return rows
    .filter((row) => {
      const first = start(row) ?? dateOf(row, "endDate");
      const last = dateOf(row, "endDate") ?? start(row);
      if (from && last && last < from) return false;
      if (to && first && first > to) return false;
      if (day && !onDay(row, day)) return false;
      if (register && registerOf(row) !== register) return false;
      const haystack = fold(`${textOf(row, "name")} ${textOf(row, "address")} ${textOf(row, "description")}`);
      return words.every((word) => haystack.includes(word));
    })
    .sort((a, b) => (start(a)?.getTime() ?? Infinity) - (start(b)?.getTime() ?? Infinity));
}

/** The Helsinki calendar day of an instant, as `YYYY-MM-DD`. */
export function dayOf(instant: Date): string {
  return instant.toLocaleDateString("sv-SE", { timeZone: ZONE });
}

/** A date input's value as the start (or, with `endOfDay`, the end) of that day; `null` when empty. */
export function inputDay(value: string, endOfDay = false): Date | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return null;
  const day = new Date(`${value}T${endOfDay ? "23:59:59" : "00:00:00"}`);
  return Number.isNaN(day.getTime()) ? null : day;
}

/**
 * The first and last Helsinki days an event takes place, as `YYYY-MM-DD`: a missing date is the
 * other one, so a one-sided event lasts one day; `null` when it has neither.
 */
function daysOf(row: Row): { first: string; last: string } | null {
  const start = dateOf(row, "startDate");
  const end = dateOf(row, "endDate");
  const first = start ?? end;
  const last = end ?? start;
  return first && last ? { first: dayOf(first), last: dayOf(last) } : null;
}

/** Whether the event takes place on `day` (`YYYY-MM-DD` in Helsinki): it has started by then and not yet ended. */
export function onDay(row: Row, day: string): boolean {
  const span = daysOf(row);
  return span !== null && span.first <= day && day <= span.last;
}

/**
 * How many events take place on each of the `days` Helsinki days from `from`'s, empty days
 * included: a festival running all month counts on every one of them, not only on the day it
 * opened (T-3029).
 */
export function perDay(rows: Row[], from: Date, days = 30): Array<{ day: string; count: number }> {
  // Calendar arithmetic on the Helsinki date, so a change of clock (25 h, 23 h) neither drops nor repeats a day.
  const [year, month, date] = dayOf(from).split("-").map(Number);
  const counts = new Map<string, number>();
  for (let offset = 0; offset < days; offset += 1) {
    counts.set(new Date(Date.UTC(year, month - 1, date + offset)).toISOString().slice(0, 10), 0);
  }
  for (const row of rows) {
    const span = daysOf(row);
    if (span === null) continue;
    for (const [day, count] of counts) {
      if (span.first <= day && day <= span.last) counts.set(day, count + 1);
    }
  }
  return [...counts].map(([day, count]) => ({ day, count }));
}

/** How many events each register publishes, most first, ties by name. */
export function byRegister(rows: Row[]): Array<{ register: string; count: number }> {
  const counts = new Map<string, number>();
  for (const row of rows) counts.set(registerOf(row), (counts.get(registerOf(row)) ?? 0) + 1);
  return [...counts.entries()]
    .map(([register, count]) => ({ register, count }))
    .sort((a, b) => b.count - a.count || a.register.localeCompare(b.register));
}

/** "22 Sep 2026, 10:00 – 23 Sep 2026, 12:00" in Helsinki's time zone. */
export function when(row: Row, locale = "en-GB"): string {
  const show = (d: Date) => d.toLocaleString(locale, { dateStyle: "medium", timeStyle: "short", timeZone: ZONE });
  const start = dateOf(row, "startDate");
  const end = dateOf(row, "endDate");
  if (start && end) return `${show(start)} – ${show(end)}`;
  if (start) return `from ${show(start)}`;
  if (end) return `until ${show(end)}`;
  return "date not given";
}

/** Whether the event can go on the map. */
export function located(row: Row): boolean {
  return pointOf(row.location) !== null;
}
