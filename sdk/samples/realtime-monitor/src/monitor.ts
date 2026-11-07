import type { Row, TemporalRow } from "@joinedcontext/sdk";

/** The level a PM2.5 reading raises an alert at, in µg/m³: the city's own setting, not a health limit. */
export const ALERT_LEVEL = 25;
/** A station with no reading for this long is shown as silent, not as its last value. */
export const SILENT_MS = 15 * 60 * 1000;
/** Points kept per station for its sparkline. */
export const KEEP = 36;

export interface Point {
  at: string;
  value: number;
}

export type History = Record<string, Point[]>;

export interface Alert {
  id: string;
  station: string;
  value: number;
  at: string;
}

function valueOf(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function keepLast(points: Point[], keep: number): Point[] {
  return points.length > keep ? points.slice(points.length - keep) : points;
}

/** The history the temporal read answered, oldest first, per entity. */
export function seed(rows: TemporalRow[], attr: string, keep = KEEP): History {
  const history: History = {};
  for (const row of rows) {
    const points = (row.series[attr] ?? [])
      .flatMap((point) => {
        const value = valueOf(point.value);
        return value === null ? [] : [{ at: point.observedAt, value }];
      })
      .sort((a, b) => a.at.localeCompare(b.at));
    if (points.length > 0) history[row.id] = keepLast(points, keep);
  }
  return history;
}

/**
 * Each row's reading added to its entity's history when it is newer than the last point there;
 * the same object back when nothing is new, so a poll that brings nothing renders nothing.
 */
export function record(history: History, rows: Row[], attr: string, keep = KEEP): History {
  let next: History | null = null;
  for (const row of rows) {
    const value = valueOf(row[attr]);
    const at = typeof row.dateObserved === "string" ? row.dateObserved : "";
    if (value === null || at === "") continue;
    const points = (next ?? history)[row.id] ?? [];
    const last = points.at(-1);
    if (last && last.at >= at) continue;
    next ??= { ...history };
    next[row.id] = keepLast([...points, { at, value }], keep);
  }
  return next ?? history;
}

function over(row: Row | undefined, attr: string, level: number): boolean {
  const value = valueOf(row?.[attr]);
  return value !== null && value > level;
}

/** Stations that crossed the level since the last poll; nothing on the first one, which has no before. */
export function crossings(before: Row[] | null, rows: Row[], attr: string, level = ALERT_LEVEL): Alert[] {
  if (before === null) return [];
  const previous = new Map(before.map((row) => [row.id, row]));
  return rows
    .filter((row) => over(row, attr, level) && !over(previous.get(row.id), attr, level))
    .map((row) => ({
      id: `${row.id}@${String(row.dateObserved ?? "")}`,
      station: typeof row.stationName === "string" ? row.stationName : row.id,
      value: valueOf(row[attr]) ?? 0,
      at: String(row.dateObserved ?? ""),
    }));
}

/** How long a station has been silent in whole minutes, or null while it reports. */
export function silentFor(row: Row, now: Date, limit = SILENT_MS): number | null {
  const at = typeof row.dateObserved === "string" ? Date.parse(row.dateObserved) : Number.NaN;
  if (Number.isNaN(at)) return null;
  const gap = now.getTime() - at;
  return gap > limit ? Math.floor(gap / 60000) : null;
}

/** An SVG polyline's points for a sparkline `width` × `height`, scaled to the points' own range. */
export function sparkline(points: Point[], width: number, height: number): string {
  if (points.length < 2) return "";
  const values = points.map((point) => point.value);
  const low = Math.min(...values);
  const span = Math.max(...values) - low || 1;
  return points
    .map((point, index) => {
      const x = (index / (points.length - 1)) * width;
      const y = height - ((point.value - low) / span) * height;
      return `${Math.round(x * 10) / 10},${Math.round(y * 10) / 10}`;
    })
    .join(" ");
}

/** The read history under what the polls already added: per entity, the older points first, no repeats. */
export function merge(older: History, newer: History, keep = KEEP): History {
  const out: History = { ...older };
  for (const [id, points] of Object.entries(newer)) {
    const base = out[id] ?? [];
    const last = base.at(-1)?.at ?? "";
    out[id] = keepLast([...base, ...points.filter((point) => point.at > last)], keep);
  }
  return out;
}
