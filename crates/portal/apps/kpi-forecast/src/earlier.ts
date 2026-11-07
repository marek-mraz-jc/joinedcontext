/**
 * Earlier forecasts against what came (T-3350): every kept forecast point whose time has passed,
 * newest first, with the reading at that time when there is one within half the model's step.
 */
import type { KeptForecast } from "./server";

export interface Due {
  madeOn: string;
  t: number;
  v: number;
  lo: number;
  hi: number;
  measured: number | null;
  within: boolean | null;
}

/** The reading nearest `t` within half `step`, or `null`. */
export function readingAt(history: ReadonlyArray<readonly [number, number]>, t: number, step: number): number | null {
  let best: readonly [number, number] | null = null;
  for (const point of history) {
    if (Math.abs(point[0] - t) <= step / 2 && (best === null || Math.abs(point[0] - t) < Math.abs(best[0] - t))) best = point;
  }
  return best === null ? null : best[1];
}

/** The points of `forecasts` due by `now`, newest first, at most `most`. */
export function dueAgainst(forecasts: KeptForecast[], history: ReadonlyArray<readonly [number, number]>, now: number, most = 10): Due[] {
  const due: Due[] = [];
  for (const forecast of forecasts) {
    for (const point of forecast.points) {
      if (point.t > now) continue;
      const measured = readingAt(history, point.t, forecast.step);
      due.push({ madeOn: forecast.madeOn, ...point, measured, within: measured === null ? null : measured >= point.lo && measured <= point.hi });
    }
  }
  return due.sort((a, b) => b.t - a.t || b.madeOn.localeCompare(a.madeOn)).slice(0, most);
}
