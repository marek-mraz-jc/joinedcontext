/**
 * The region's events as the dashboard reads them (T-4897): rows of the Event model of
 * `bbsk-podujatia` (T-4895) as the pipelines of T-4896 write them, the filters a person sets, the
 * month the calendar draws and the calendar link a person subscribes to. Pure functions, so every
 * rule is tested without a screen.
 *
 * Every day is a day in Slovakia (Europe/Bratislava): an event at 23:30 there is on that day for a
 * reader abroad too, and the calendar's `from`/`to` are the same days.
 */
import { format } from "@joinedcontext/sdk";
import type { Cell, RichCell, RichRow } from "@joinedcontext/sdk";

export const ZONE = "Europe/Bratislava";

export interface EventItem {
  id: string;
  name: string;
  /** The start as an instant; a date-only start is that day's midnight in Slovakia. */
  start: Date;
  /** `YYYY-MM-DD` in Slovakia of the start and the end (the start's when the source gave none). */
  startDay: string;
  endDay: string;
  /** The source gave dates without a time. */
  allDay: boolean;
  /** The end as an instant, or `null` when the source gave none. */
  end: Date | null;
  town: string | null;
  venue: string | null;
  address: string | null;
  category: string | null;
  description: string | null;
  /** The organiser's page of the event, only an `http(s)` address. */
  url: string | null;
  provider: string | null;
  /** `[longitude, latitude]`; the model requires one, a row without it is still listed. */
  coordinates: [number, number] | null;
}

export interface Filters {
  town: string;
  category: string;
  /** `YYYY-MM-DD` or empty. */
  from: string;
  to: string;
  text: string;
}

export const NO_FILTERS: Filters = { town: "", category: "", from: "", to: "", text: "" };

const DATE_ONLY = /^\d{4}-\d{2}-\d{2}$/;

function first(cell: RichCell | RichCell[] | undefined): RichCell | undefined {
  return Array.isArray(cell) ? cell[0] : cell;
}

function text(row: RichRow, attr: string, locale = "sk"): string | null {
  const cell = first(row.cells[attr]);
  const map = cell?.languageMap;
  const value = map ? (map[locale] ?? map.sk ?? Object.values(map)[0]) : cell?.value;
  if (Array.isArray(value)) return value.filter((part) => typeof part === "string").join(", ") || null;
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

/** `YYYY-MM-DD` of an instant in Slovakia. */
export function dayOf(instant: Date): string {
  // en-CA writes the date as YYYY-MM-DD.
  return new Intl.DateTimeFormat("en-CA", { timeZone: ZONE, year: "numeric", month: "2-digit", day: "2-digit" }).format(instant);
}

/** A date or a date-time as the model holds it, or `null` when it is neither. */
export function parseWhen(value: string | null): { instant: Date; day: string; allDay: boolean } | null {
  if (value === null) return null;
  if (DATE_ONLY.test(value)) {
    const instant = new Date(`${value}T00:00:00Z`);
    return Number.isNaN(instant.getTime()) ? null : { instant, day: value, allDay: true };
  }
  const instant = new Date(value);
  return Number.isNaN(instant.getTime()) ? null : { instant, day: dayOf(instant), allDay: false };
}

/** An address a person may follow: only `http` and `https`, so a `javascript:` value never runs. */
export function safeLink(value: string | null): string | null {
  if (value === null) return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:" ? url.href : null;
  } catch {
    return null;
  }
}

function point(row: RichRow): [number, number] | null {
  const value = first(row.cells.location)?.value as { type?: unknown; coordinates?: unknown } | undefined;
  if (value?.type !== "Point" || !Array.isArray(value.coordinates)) return null;
  const [lon, lat] = value.coordinates;
  return typeof lon === "number" && typeof lat === "number" && Math.abs(lon) <= 180 && Math.abs(lat) <= 90 ? [lon, lat] : null;
}

/** One row as an event, or `null` when it has no name or no start the dashboard can place. */
export function eventOf(row: RichRow, locale: string): EventItem | null {
  const name = text(row, "name", locale);
  const start = parseWhen(text(row, "startDate"));
  if (name === null || start === null) return null;
  const end = parseWhen(text(row, "endDate"));
  // An end before its start is a source's slip: the event keeps its start day.
  const endDay = end !== null && end.day >= start.day ? end.day : start.day;
  const address = first(row.cells.address)?.value;
  const addressObject = address && typeof address === "object" && !Array.isArray(address) ? (address as Record<string, unknown>) : null;
  const locality = addressObject?.addressLocality;
  return {
    id: row.id,
    name,
    start: start.instant,
    startDay: start.day,
    endDay,
    allDay: start.allDay,
    end: end?.instant ?? null,
    town: typeof locality === "string" && locality.trim() !== "" ? locality.trim() : null,
    venue: text(row, "venue", locale),
    address: addressObject ? format(address as Cell) || null : null,
    category: text(row, "eventCategory", locale),
    description: text(row, "description", locale),
    url: safeLink(text(row, "url")),
    provider: text(row, "dataProvider", locale),
    coordinates: point(row),
  };
}

/** Text folded for search: lower case, without diacritics, so "banska" finds "Banská". */
export function folded(value: string): string {
  return value.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/** Whether an event passes every filter: a date range keeps the events on any of its days. */
export function matches(event: EventItem, filters: Filters): boolean {
  if (filters.town !== "" && event.town !== filters.town) return false;
  if (filters.category !== "" && event.category !== filters.category) return false;
  if (filters.from !== "" && event.endDay < filters.from) return false;
  if (filters.to !== "" && event.startDay > filters.to) return false;
  const words = folded(filters.text).split(/\s+/).filter((word) => word !== "");
  if (words.length === 0) return true;
  const haystack = folded(
    [event.name, event.venue, event.address, event.town, event.category, event.provider, event.description].filter(Boolean).join(" "),
  );
  return words.every((word) => haystack.includes(word));
}

/** Soonest first, then by name. */
export function bySoonest(a: EventItem, b: EventItem): number {
  return a.start.getTime() - b.start.getTime() || a.name.localeCompare(b.name, "sk");
}

/** The distinct values of one field, in Slovak alphabetical order: the choices of a filter. */
export function choices(events: EventItem[], field: "town" | "category"): string[] {
  const values = new Set<string>();
  for (const event of events) {
    const value = event[field];
    if (value !== null) values.add(value);
  }
  return [...values].sort((a, b) => a.localeCompare(b, "sk"));
}

/** A date range that cannot hold a day: `from` after `to`. */
export function rangeInvalid(filters: Filters): boolean {
  return filters.from !== "" && filters.to !== "" && filters.from > filters.to;
}

/**
 * Where the platform's public API answers: `api.{domain}` for an App on `{name}.apps.{domain}`
 * (ADR-N-037), else the page's own origin, which serves the same `/api/endpoint/` (T-2670).
 */
export function apiOrigin(location: { protocol: string; host: string }): string {
  const apps = /^[a-z0-9-]+\.apps\.(.+)$/.exec(location.host);
  return apps ? `${location.protocol}//api.${apps[1]}` : `${location.protocol}//${location.host}`;
}

/**
 * The iCalendar link of the endpoint (EP-125, User-Guide 16): the whole region, or its `from`/`to`
 * days. The calendar serves no other filter today (T-4901 brings NGSI-LD `q` to it), so a town,
 * a category or a search never goes into the link where it would be ignored.
 */
export function calendarUrl(origin: string, slug: string, range: { from: string; to: string } = { from: "", to: "" }): string {
  const url = new URL(`/api/endpoint/${encodeURIComponent(slug)}/ical/v1/events.ics`, origin);
  if (DATE_ONLY.test(range.from)) url.searchParams.set("from", range.from);
  if (DATE_ONLY.test(range.to)) url.searchParams.set("to", range.to);
  return url.href;
}

/** The same link as `webcal://`, which opens the reader's calendar application from a browser. */
export function webcalUrl(url: string): string {
  return url.replace(/^https?:\/\//, "webcal://");
}

/** `YYYY-MM` of a day. */
export function monthOf(day: string): string {
  return day.slice(0, 7);
}

/** The month before or after `YYYY-MM`. */
export function shiftMonth(month: string, by: number): string {
  const [year, index] = month.split("-").map(Number);
  const moved = new Date(Date.UTC(year, index - 1 + by, 1));
  return moved.toISOString().slice(0, 7);
}

/** The days a month view draws: whole weeks from Monday, the days outside the month included. */
export function monthDays(month: string): string[] {
  const [year, index] = month.split("-").map(Number);
  const firstDay = new Date(Date.UTC(year, index - 1, 1));
  const lead = (firstDay.getUTCDay() + 6) % 7;
  const length = new Date(Date.UTC(year, index, 0)).getUTCDate();
  const cells = Math.ceil((lead + length) / 7) * 7;
  return Array.from({ length: cells }, (_, at) => new Date(Date.UTC(year, index - 1, 1 - lead + at)).toISOString().slice(0, 10));
}

/** The events on one day, a several-day event on each of its days. */
export function onDay(events: EventItem[], day: string): EventItem[] {
  return events.filter((event) => event.startDay <= day && day <= event.endDay);
}

/** The events as GeoJSON points, which is the one thing the map is given. */
export function featuresOf(events: EventItem[], picked: string | null) {
  return {
    type: "FeatureCollection" as const,
    features: events
      .filter((event) => event.coordinates !== null)
      .map((event) => ({
        type: "Feature" as const,
        id: event.id,
        geometry: { type: "Point" as const, coordinates: event.coordinates as [number, number] },
        properties: { id: event.id, picked: event.id === picked },
      })),
  };
}
