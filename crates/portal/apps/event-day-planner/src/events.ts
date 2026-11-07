import { format, pointOf } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { ZONE } from "./i18n";
import type { PlanEvent } from "./planner";

/** The one entity type the app's data need names (AP-04). */
export const EVENT = "Event";

/** The attributes the page reads, exactly those app.yaml names. */
export const ATTRS = ["name", "description", "startDate", "endDate", "eventStatus", "address", "location", "source"];

const HOUR = 3_600_000;

/** The Helsinki calendar day of an instant, as `YYYY-MM-DD`. */
export function dayOf(instant: Date): string {
  return instant.toLocaleDateString("sv-SE", { timeZone: ZONE });
}

/** How far Helsinki's clock is ahead of UTC at `ms`, in milliseconds (2 or 3 hours). */
export function zoneOffset(ms: number): number {
  const parts = Object.fromEntries(
    new Intl.DateTimeFormat("en-US", {
      timeZone: ZONE,
      hourCycle: "h23",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    })
      .formatToParts(new Date(ms))
      .map((part) => [part.type, part.value]),
  );
  const asUtc = Date.UTC(Number(parts.year), Number(parts.month) - 1, Number(parts.day), Number(parts.hour), Number(parts.minute), Number(parts.second));
  return asUtc - Math.floor(ms / 1000) * 1000;
}

/** The instants a Helsinki day `YYYY-MM-DD` starts and ends, as epoch milliseconds; `null` for no day. */
export function dayBounds(day: string): [number, number] | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  if (!match) return null;
  const [year, month, date] = [Number(match[1]), Number(match[2]), Number(match[3])];
  const midnight = (d: number) => {
    // Four digits of year: always an instant JavaScript can hold.
    const guess = Date.UTC(year, month - 1, d);
    // Twice: the offset at the guess, then at the corrected instant (a day the clock changes).
    const first = guess - zoneOffset(guess);
    return guess - zoneOffset(first);
  };
  const start = midnight(date);
  const end = midnight(date + 1);
  if (dayOf(new Date(start)) !== day) return null;
  return [start, end];
}

/** A date attribute as epoch milliseconds, or `null` when missing or not a date. */
export function timeOf(row: Row, attr: "startDate" | "endDate"): number | null {
  const raw = row[attr];
  if (typeof raw !== "string") return null;
  const parsed = Date.parse(raw);
  return Number.isNaN(parsed) ? null : parsed;
}

/** A text attribute in the reader's language (the SDK resolves a language map). */
export function textOf(row: Row, attr: string): string {
  return format(row[attr]).trim();
}

/** The end of an event's id: short enough for the address, unique in the city's feed. */
export function localOf(id: string): string {
  return id.slice(id.lastIndexOf(":") + 1);
}

/** The event's name, else the end of its id. */
export function nameOf(row: Row): string {
  return textOf(row, "name") || localOf(row.id);
}

export function cancelled(row: Row): boolean {
  return textOf(row, "eventStatus") === "EventCancelled";
}

/** The event's link to its source, only when it is an `https:` address. */
export function sourceOf(row: Row): string {
  const source = textOf(row, "source");
  return /^https:\/\/[^\s]+$/.test(source) ? source : "";
}

/** The query of the events that have not ended before `from` (NGSI-LD q, a DateTime is unquoted). */
export function upcomingQuery(from: number): string {
  return `endDate>=${new Date(from).toISOString().replace(/\.\d{3}Z$/, "Z")}`;
}

/** Whether the event takes place during [start, end): it starts before the end and ends after the start. */
export function during(row: Row, [start, end]: [number, number]): boolean {
  const first = timeOf(row, "startDate");
  if (first === null) return false;
  const last = Math.max(timeOf(row, "endDate") ?? first, first);
  return first < end && (last > start || (last === first && first >= start));
}

const fold = (value: string) => value.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

/**
 * The day's events a reader asked for, soonest first: taking place that day, not cancelled unless
 * already chosen, holding every word of `query` in name, place or description (case and accents
 * ignored), and starting in `hour` (Helsinki, 0–23) when given.
 */
export function eventsOfDay(rows: Row[], bounds: [number, number], query = "", hour: number | null = null): Row[] {
  const words = fold(query).split(/\s+/).filter(Boolean);
  return rows
    .filter((row) => {
      if (!during(row, bounds)) return false;
      if (hour !== null && startHour(row, bounds) !== hour) return false;
      const haystack = fold(`${textOf(row, "name")} ${textOf(row, "address")} ${textOf(row, "description")}`);
      return words.every((word) => haystack.includes(word));
    })
    .sort((a, b) => (timeOf(a, "startDate") ?? 0) - (timeOf(b, "startDate") ?? 0) || a.id.localeCompare(b.id));
}

/** The Helsinki hour an event starts that day; 0 for one that began before the day. */
export function startHour(row: Row, [start]: [number, number]): number {
  const first = timeOf(row, "startDate") ?? start;
  return first <= start ? 0 : Math.floor((first - start - zoneOffset(start) + zoneOffset(first)) / HOUR);
}

/** How many of the day's events start in each hour, 0 to 23. */
export function perHour(rows: Row[], bounds: [number, number]): number[] {
  const counts = Array.from({ length: 24 }, () => 0);
  for (const row of rows) counts[Math.min(23, Math.max(0, startHour(row, bounds)))] += 1;
  return counts;
}

/** The events as the planner reads them, each window cut to the day. */
export function planEvents(rows: Row[], [start, end]: [number, number]): PlanEvent[] {
  return rows.map((row) => {
    const first = timeOf(row, "startDate");
    const last = timeOf(row, "endDate");
    const at = pointOf(row.location);
    return {
      id: localOf(row.id),
      name: nameOf(row),
      address: textOf(row, "address"),
      start: first === null ? null : Math.max(first, start),
      end: first === null ? null : Math.min(Math.max(last ?? first, first), end),
      lon: at ? at[0] : null,
      lat: at ? at[1] : null,
    };
  });
}
