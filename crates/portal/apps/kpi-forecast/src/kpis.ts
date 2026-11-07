/**
 * The indicators as the page reads them: the KPI entities of `helsinki-kpi`, their history from the
 * temporal read, and the view in the address (`?kpi=…&days=30&odd=1`).
 */
import type { Row, TemporalRow } from "@joinedcontext/sdk";
import type { SeriesInput } from "./analysis";

export const KPI = "KeyPerformanceIndicator";
/** What the KPI entities carry on dev (2026-10-08): the value with its time, its name, its formula and its source. */
export const ATTRS = ["name", "currentValue", "calculationFormula", "source"];
/** The history windows offered, in days; the App's grant reads at most the longest (temporalQ P90D). */
export const WINDOWS = [7, 30, 90] as const;
export type Window = (typeof WINDOWS)[number];
export const DEFAULT_WINDOW: Window = 30;
/** The most instances the temporal read asks for per indicator: the model's clock holds 2000 steps. */
export const LAST_N = 2000;

export interface ViewState {
  /** The indicator shown in full: its id, or `null` for the first by name. */
  kpi: string | null;
  days: Window;
  /** Only the indicators with a point that looks wrong. */
  odd: boolean;
}

export const EMPTY: ViewState = { kpi: null, days: DEFAULT_WINDOW, odd: false };

export function readView(search: string): ViewState {
  const params = new URLSearchParams(search);
  const days = Number(params.get("days"));
  return {
    kpi: params.get("kpi") || null,
    days: (WINDOWS as readonly number[]).includes(days) ? (days as Window) : DEFAULT_WINDOW,
    odd: params.get("odd") === "1",
  };
}

/** The address for a view, every other parameter (the language) kept; defaults are left out. */
export function writeView(search: string, view: ViewState): string {
  const params = new URLSearchParams(search);
  for (const key of ["kpi", "days", "odd"]) params.delete(key);
  if (view.kpi) params.set("kpi", view.kpi);
  if (view.days !== DEFAULT_WINDOW) params.set("days", String(view.days));
  if (view.odd) params.set("odd", "1");
  const text = params.toString();
  return text ? `?${text}` : "";
}

/** A number from a cell: the KPI values are numbers, a text that reads as one counts too. */
export function numberOf(value: unknown): number | null {
  if (typeof value === "number") return Number.isFinite(value) ? value : null;
  if (typeof value === "string" && value.trim() !== "") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

/**
 * Each indicator's points, from the temporal read. A row carries its current value without its
 * time, so a value the history does not hold is shown beside the series and never placed on it.
 */
export function toSeries(rows: Row[], history: TemporalRow[]): SeriesInput[] {
  const byId = new Map(history.map((h) => [h.id, h]));
  return rows.map((row) => {
    const points: { t: number; v: number }[] = [];
    for (const point of byId.get(row.id)?.series.currentValue ?? []) {
      const t = Date.parse(point.observedAt);
      const v = numberOf(point.value);
      if (Number.isFinite(t) && v !== null) points.push({ t, v });
    }
    return { id: row.id, points };
  });
}

/** The indicator's own name in its id, `bike-network-capacity` of `urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:bike-network-capacity`. */
export function shortId(id: string): string {
  return id.slice(id.lastIndexOf(":") + 1);
}
