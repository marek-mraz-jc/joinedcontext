import type { Row } from "@joinedcontext/sdk";

/** An event placed in time: where it starts and where it ends (its start when it has no end). */
export interface Span {
  row: Row;
  start: Date;
  end: Date;
}

const DAY = 24 * 60 * 60 * 1000;

export function startOfDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

export function addDays(date: Date, days: number): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() + days);
}

/** The local calendar day, so an evening event does not slide into the next day's cell. */
export function dayKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

/** Weeks start on Monday (ISO 8601). */
export function startOfWeek(date: Date): Date {
  return addDays(startOfDay(date), -((date.getDay() + 6) % 7));
}

export function weekOf(anchor: Date): Date[] {
  const monday = startOfWeek(anchor);
  return Array.from({ length: 7 }, (_, day) => addDays(monday, day));
}

/** Whole weeks, Monday to Sunday, from the week of the 1st to the week of the last day. */
export function monthGrid(anchor: Date): Date[][] {
  const first = new Date(anchor.getFullYear(), anchor.getMonth(), 1);
  const last = new Date(anchor.getFullYear(), anchor.getMonth() + 1, 0);
  const weeks: Date[][] = [];
  for (let monday = startOfWeek(first); monday <= last; monday = addDays(monday, 7)) weeks.push(weekOf(monday));
  return weeks;
}

function dateOf(value: unknown): Date | null {
  if (typeof value !== "string" || value === "") return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** Every event with a readable start, oldest first; an end before the start is read as no end. */
export function spans(rows: Row[]): Span[] {
  return rows
    .flatMap((row) => {
      const start = dateOf(row.startDate);
      if (!start) return [];
      const end = dateOf(row.endDate);
      return [{ row, start, end: end && end >= start ? end : start }];
    })
    .sort((a, b) => a.start.getTime() - b.start.getTime());
}

/** The events that take place on a day, a multi-day one on each of its days. */
export function onDay(list: Span[], day: Date): Span[] {
  const from = startOfDay(day).getTime();
  const to = from + DAY;
  return list.filter((span) => span.start.getTime() < to && span.end.getTime() >= from);
}

/** What is still to come (or running now), soonest first, and what is over, latest first. */
export function timeline(list: Span[], now: Date): { upcoming: Span[]; earlier: Span[] } {
  const upcoming = list.filter((span) => span.end >= now);
  const earlier = list.filter((span) => span.end < now).reverse();
  return { upcoming, earlier };
}

/** Consecutive spans grouped by the day they start on, keeping the order they came in. */
export function byStartDay(list: Span[]): { day: Date; spans: Span[] }[] {
  const groups: { day: Date; spans: Span[] }[] = [];
  for (const span of list) {
    const last = groups.at(-1);
    if (last && dayKey(last.day) === dayKey(span.start)) last.spans.push(span);
    else groups.push({ day: startOfDay(span.start), spans: [span] });
  }
  return groups;
}
