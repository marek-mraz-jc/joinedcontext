/**
 * The alerts as the analysis reads them, and the filters as the address carries them, so a view
 * is a link a person can send (`?from=2026-09-01&kind=roadWorks&day=0&hour=7&lang=en`).
 */
import type { Row } from "@joinedcontext/sdk";
import type { AlertInput, Filter } from "./analysis";
import { ZONE } from "./i18n";

/** The one entity type the App's data need names (AP-04). */
export const ALERT = "Alert";
/** The attributes it reads, as app.yaml names them. */
export const ATTRS = ["name", "subCategory", "address", "dateIssued", "validFrom", "location"];

/** When an alert starts: its `validFrom`, else when it was issued; `null` when it says neither. */
export function startOf(row: Row): number | null {
  for (const attr of ["validFrom", "dateIssued"]) {
    const raw = row[attr];
    if (typeof raw === "string") {
      const ms = Date.parse(raw);
      if (!Number.isNaN(ms)) return ms;
    }
  }
  return null;
}

/** A row as the Rust module takes it: the geometry as the endpoint answers it. */
export function toInput(row: Row): AlertInput {
  const sub = row.subCategory;
  return {
    id: row.id,
    geometry: row.location ?? null,
    time: startOf(row),
    subCategory: typeof sub === "string" ? sub : "",
  };
}

export interface ViewState {
  /** Days on Helsinki's calendar, `YYYY-MM-DD`, both inclusive; empty is no limit. */
  from: string;
  to: string;
  kinds: string[];
  weekday: number | null;
  hour: number | null;
}

export const EMPTY: ViewState = { from: "", to: "", kinds: [], weekday: null, hour: null };

const DAY = /^\d{4}-\d{2}-\d{2}$/;

const intIn = (raw: string | null, max: number): number | null => {
  if (raw === null || !/^\d{1,2}$/.test(raw)) return null;
  const value = Number(raw);
  return value <= max ? value : null;
};

/** The view an address asks for; anything malformed in it is left out, never guessed. */
export function readView(search: string): ViewState {
  const params = new URLSearchParams(search);
  const from = params.get("from") ?? "";
  const to = params.get("to") ?? "";
  const weekday = intIn(params.get("day"), 6);
  const hour = intIn(params.get("hour"), 23);
  return {
    from: DAY.test(from) ? from : "",
    to: DAY.test(to) ? to : "",
    kinds: (params.get("kind") ?? "").split(",").filter((kind) => /^[A-Za-z_/]{1,40}$/.test(kind)),
    // An hour of the week is a pair: one half alone picks nothing.
    weekday: weekday !== null && hour !== null ? weekday : null,
    hour: weekday !== null && hour !== null ? hour : null,
  };
}

/** The address of a view, keeping every other parameter (`lang`) as it was. */
export function writeView(search: string, view: ViewState): string {
  const params = new URLSearchParams(search);
  const set = (name: string, value: string) => (value ? params.set(name, value) : params.delete(name));
  set("from", view.from);
  set("to", view.to);
  set("kind", view.kinds.join(","));
  set("day", view.weekday === null ? "" : String(view.weekday));
  set("hour", view.hour === null ? "" : String(view.hour));
  const text = params.toString();
  return text ? `?${text}` : "";
}

/**
 * Milliseconds of midnight in Helsinki starting a day: Helsinki is UTC+2 or +3, so midnight is
 * 22:00 or 21:00 UTC the day before; the offset is read from the zone's own clock.
 */
export function helsinkiMidnight(day: string): number | undefined {
  if (!DAY.test(day)) return undefined;
  const utcMidnight = Date.parse(`${day}T00:00:00Z`);
  const parts = new Intl.DateTimeFormat("en-GB", { timeZone: ZONE, hour: "2-digit", hourCycle: "h23" }).formatToParts(utcMidnight);
  const hour = Number(parts.find((part) => part.type === "hour")?.value ?? 0);
  return utcMidnight - hour * 3_600_000;
}

/** The filter the Rust module applies: `to` is the end of its day, so the day itself is kept. */
export function filterOf(view: ViewState): Filter {
  const from = helsinkiMidnight(view.from);
  const until = view.to ? helsinkiMidnight(nextDay(view.to)) : undefined;
  return {
    ...(from !== undefined ? { from } : {}),
    ...(until !== undefined ? { to: until } : {}),
    ...(view.kinds.length > 0 ? { subCategories: view.kinds } : {}),
    ...(view.weekday !== null && view.hour !== null ? { weekday: view.weekday, hour: view.hour } : {}),
  };
}

function nextDay(day: string): string {
  const next = new Date(Date.parse(`${day}T12:00:00Z`) + 86_400_000);
  return next.toISOString().slice(0, 10);
}
