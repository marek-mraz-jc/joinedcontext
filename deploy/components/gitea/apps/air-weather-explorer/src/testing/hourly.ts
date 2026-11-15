/**
 * A station's readings in the fixture as the App's server keeps them (T-3348): hourly means,
 * humidity in per cent. For the page's tests and the bundle's local Playwright run alike.
 */
import type { Hourly } from "../server";

const HOUR = 3_600_000;

/** One station's readings as hourly means from `from` on. */
export function hourlyOf(history: ReadonlyArray<Record<string, unknown>>, id: string, from: number): Hourly {
  const entity = history.find((e) => e.id === id);
  const out: Hourly = {};
  for (const [attr, property] of Object.entries(entity ?? {})) {
    const values = (property as { values?: unknown }).values;
    if (!Array.isArray(values)) continue;
    const sums = new Map<number, [number, number]>();
    for (const entry of values) {
      const [value, at] = entry as [unknown, unknown];
      const ms = typeof at === "string" ? Date.parse(at) : NaN;
      if (typeof value !== "number" || !Number.isFinite(value) || Number.isNaN(ms)) continue;
      const hour = Math.floor(ms / HOUR) * HOUR;
      const [sum, n] = sums.get(hour) ?? [0, 0];
      sums.set(hour, [sum + (attr === "relativeHumidity" ? value * 100 : value), n + 1]);
    }
    const points = [...sums].filter(([hour]) => hour >= from).sort(([a], [b]) => a - b).map(([hour, [sum, n]]) => [hour, sum / n] as [number, number]);
    if (points.length > 0) out[attr] = points;
  }
  return out;
}
