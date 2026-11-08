/**
 * Grafana's time words (`now`, `now-6h`, `now-1d/d`, `now/fQ`, epoch milliseconds, ISO 8601) as the
 * instants a chart's axis spans, and the quick ranges and refresh intervals Grafana 12.4's pickers
 * offer, in its order (tests/fixtures/grafana-reference). Days, weeks, months, quarters and years
 * are calendar steps in the reader's time zone, weeks start on Sunday and the fiscal year in
 * January, as Grafana's defaults are.
 */

const FIXED_MS: Record<string, number> = { s: 1000, m: 60_000, h: 3_600_000 };

/** `at` moved by `amount` of `unit`, the calendar way for days and longer (a month keeps its day, clamped). */
function add(at: number, amount: number, unit: string): number {
  if (unit in FIXED_MS) return at + amount * FIXED_MS[unit];
  const date = new Date(at);
  if (unit === "d" || unit === "w") {
    date.setDate(date.getDate() + amount * (unit === "w" ? 7 : 1));
    return date.getTime();
  }
  const months = amount * (unit === "y" ? 12 : unit === "Q" ? 3 : 1);
  const day = date.getDate();
  date.setDate(1);
  date.setMonth(date.getMonth() + months);
  date.setDate(Math.min(day, new Date(date.getFullYear(), date.getMonth() + 1, 0).getDate()));
  return date.getTime();
}

/** The first instant of the `unit` holding `at`; the fiscal units with a year from January. */
function startOf(at: number, unit: string): number {
  const date = new Date(at);
  const [y, M, d] = [date.getFullYear(), date.getMonth(), date.getDate()];
  switch (unit) {
    case "y":
    case "fy":
      return new Date(y, 0, 1).getTime();
    case "Q":
    case "fQ":
      return new Date(y, M - (M % 3), 1).getTime();
    case "M":
      return new Date(y, M, 1).getTime();
    case "w":
      return new Date(y, M, d - date.getDay()).getTime();
    case "d":
      return new Date(y, M, d).getTime();
    case "h":
      return new Date(y, M, d, date.getHours()).getTime();
    case "m":
      return new Date(y, M, d, date.getHours(), date.getMinutes()).getTime();
    default:
      return Math.floor(at / 1000) * 1000;
  }
}

/** The last millisecond of the `unit` holding `at`. */
function endOf(at: number, unit: string): number {
  const step = unit === "fy" ? "y" : unit === "fQ" ? "Q" : unit;
  return add(startOf(at, unit), 1, step) - 1;
}

/**
 * The instant `text` names at `now`; `null` for a word Grafana does not use. `roundUp` reads a
 * rounding (`/d`) as the end of that unit, as Grafana reads the range's `to`.
 */
export function resolveTime(text: string, now: number, roundUp = false): number | null {
  const trimmed = text.trim();
  if (/^\d+$/.test(trimmed)) return Number(trimmed);
  if (trimmed.startsWith("now")) {
    let at = now;
    const steps = /([+-])(\d+)([smhdwMyQ])|\/(fQ|fy|[smhdwMyQ])/g;
    let read = 3;
    for (let step = steps.exec(trimmed); step && step.index === read; step = steps.exec(trimmed)) {
      const [whole, sign, amount, unit, round] = step;
      at = round ? (roundUp ? endOf(at, round) : startOf(at, round)) : add(at, (sign === "-" ? -1 : 1) * Number(amount), unit);
      read += whole.length;
    }
    return read === trimmed.length ? at : null;
  }
  // An absolute time as Grafana's picker writes it, `2026-11-08 06:00:00`, is local time.
  const parsed = Date.parse(/^\d{4}-\d\d-\d\d \d\d:\d\d(:\d\d)?$/.test(trimmed) ? trimmed.replace(" ", "T") : trimmed);
  return Number.isNaN(parsed) ? null : parsed;
}

/** One line of the quick range list: Grafana's words and the key of its label in the locales. */
export interface QuickRange {
  from: string;
  to: string;
  key: string;
}

const quick = (from: string, to: string, key: string): QuickRange => ({ from, to, key });

/** Grafana 12.4's quick ranges, in its order. */
export const QUICK_RANGES: readonly QuickRange[] = [
  ...["5m", "15m", "30m", "1h", "3h", "6h", "12h", "24h", "2d", "7d", "30d", "90d", "6M", "1y", "2y", "5y"].map((span) => quick(`now-${span}`, "now", `last${span}`)),
  quick("now-1d/d", "now-1d/d", "yesterday"),
  quick("now-2d/d", "now-2d/d", "dayBeforeYesterday"),
  quick("now-7d/d", "now-7d/d", "thisDayLastWeek"),
  quick("now-1w/w", "now-1w/w", "previousWeek"),
  quick("now-1M/M", "now-1M/M", "previousMonth"),
  quick("now-1Q/fQ", "now-1Q/fQ", "previousFiscalQuarter"),
  quick("now-1y/y", "now-1y/y", "previousYear"),
  quick("now-1y/fy", "now-1y/fy", "previousFiscalYear"),
  quick("now/d", "now/d", "today"),
  quick("now/d", "now", "todaySoFar"),
  quick("now/w", "now/w", "thisWeek"),
  quick("now/w", "now", "thisWeekSoFar"),
  quick("now/M", "now/M", "thisMonth"),
  quick("now/M", "now", "thisMonthSoFar"),
  quick("now/y", "now/y", "thisYear"),
  quick("now/y", "now", "thisYearSoFar"),
  quick("now/fQ", "now", "thisFiscalQuarterSoFar"),
  quick("now/fQ", "now/fQ", "thisFiscalQuarter"),
  quick("now/fy", "now", "thisFiscalYearSoFar"),
  quick("now/fy", "now/fy", "thisFiscalYear"),
];

/** The quick range `from`..`to` is, if it is one. */
export function quickRange(from: string, to: string): QuickRange | undefined {
  return QUICK_RANGES.find((range) => range.from === from && range.to === to);
}

/** Grafana 12.4's refresh intervals: `""` is off, `auto` follows the range and the screen width. */
export const REFRESHES = ["", "auto", "5s", "10s", "30s", "1m", "5m", "15m", "30m", "1h", "2h", "1d"] as const;

/** Grafana's rangeUtil.roundInterval: a raw interval to the nearest of its round steps, in ms. */
export function roundInterval(ms: number): number {
  const steps: [number, number][] = [
    [15, 10], [35, 20], [75, 50], [150, 100], [350, 200], [750, 500], [1500, 1000], [3500, 2000],
    [7500, 5000], [12500, 10000], [17500, 15000], [25000, 20000], [45000, 30000], [90000, 60000],
    [210000, 120000], [450000, 300000], [750000, 600000], [1050000, 900000], [1500000, 1200000],
    [2700000, 1800000], [5400000, 3600000], [9000000, 7200000], [16200000, 10800000],
    [32400000, 21600000], [86400000, 43200000], [604800000, 86400000], [1814400000, 604800000],
    [3628800000, 2592000000],
  ];
  return steps.find(([below]) => ms < below)?.[1] ?? 31536000000;
}

/**
 * A refresh interval (`30s`, `5m`, `1d`) in milliseconds; 0 for off or a word it does not know.
 * `auto` is Grafana's: the range over the screen width, rounded, at least 5 s.
 */
export function refreshMs(text: string | undefined, span = 0, width = 2000): number {
  if (text === "auto") return Math.max(5000, roundInterval(span / Math.max(1, width)));
  const match = /^(\d+)([smhd])$/.exec(text ?? "");
  return match ? Number(match[1]) * (match[2] === "d" ? 86_400_000 : FIXED_MS[match[2]]) : 0;
}

/** `instant` as Grafana's picker writes an absolute time, `2026-11-08 06:00:00`, in local time. */
export function absoluteWords(instant: number): string {
  const date = new Date(instant);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${two(date.getMonth() + 1)}-${two(date.getDate())} ${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}`;
}

/** A Grafana span (`1h`, `2d`) as its amount and unit, as `timeFrom` and `timeShift` write them. */
export function spanParts(text: string | undefined): { n: number; unit: string } | null {
  const match = /^(\d+)([smhdwMy])$/.exec(text?.trim() ?? "");
  return match ? { n: Number(match[1]), unit: match[2] } : null;
}

/** A panel's own range, Grafana's way: `timeFrom` only under a relative dashboard range, then `timeShift` back. */
export function panelRange(
  panel: { timeFrom?: string; timeShift?: string },
  asked: { from: string; to: string },
  now: number,
): { from: number; to: number; timeFrom?: string; timeShift?: string } {
  let from = resolveTime(asked.from, now) ?? now - 6 * 3_600_000;
  let to = resolveTime(asked.to, now, true) ?? now;
  const relative = asked.from.startsWith("now") && asked.to.startsWith("now");
  const ownFrom = spanParts(panel.timeFrom);
  const ownShift = spanParts(panel.timeShift);
  if (ownFrom && relative) {
    from = add(now, -ownFrom.n, ownFrom.unit);
    to = now;
  }
  if (ownShift) {
    from = add(from, -ownShift.n, ownShift.unit);
    to = add(to, -ownShift.n, ownShift.unit);
  }
  return { from, to, ...(ownFrom && relative ? { timeFrom: panel.timeFrom } : {}), ...(ownShift ? { timeShift: panel.timeShift } : {}) };
}
