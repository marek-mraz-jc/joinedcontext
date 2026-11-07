import { currentTokens } from "@joinedcontext/sdk";
import type { DesignTokens } from "@joinedcontext/sdk";
import { ZONE } from "./events";

const GRID = { containLabel: true, left: 8, right: 24, top: 24, bottom: 8 };

/** The colour of each register: the palette of the design tokens in the order the registers are ranked. */
export function registerColours(registers: string[], tokens?: DesignTokens): Map<string, string> {
  const palette = (tokens ?? currentTokens()).chart.palette;
  return new Map(registers.map((register, index) => [register, palette[index % palette.length]]));
}

/** "20 Oct" for a `YYYY-MM-DD` day. */
export function dayLabel(day: string, locale = "en-GB"): string {
  return new Date(`${day}T12:00:00Z`).toLocaleDateString(locale, { day: "numeric", month: "short", timeZone: ZONE });
}

/**
 * Events taking place on each day, one bar per day with its `YYYY-MM-DD` as the category a click
 * selects; `null` when no day has one, so the card says so instead of drawing an empty axis.
 */
export function perDayOption(series: Array<{ day: string; count: number }>, tokens?: DesignTokens): Record<string, unknown> | null {
  if (series.every((point) => point.count === 0)) return null;
  const t = tokens ?? currentTokens();
  return {
    color: t.chart.palette,
    tooltip: { trigger: "axis" },
    grid: GRID,
    xAxis: {
      type: "category",
      data: series.map((point) => point.day),
      axisLabel: { formatter: (day: string) => dayLabel(day) },
    },
    yAxis: { type: "value", name: "Events", minInterval: 1 },
    series: [{ type: "bar", name: "Events", itemStyle: { color: t.color.accent }, data: series.map((point) => point.count) }],
  };
}

/** Events per register as horizontal bars, most first, each in its register's colour; `null` for none. */
export function registerOption(
  counts: Array<{ register: string; count: number }>,
  colours: Map<string, string>,
  tokens?: DesignTokens,
): Record<string, unknown> | null {
  if (counts.length === 0) return null;
  const t = tokens ?? currentTokens();
  return {
    color: t.chart.palette,
    tooltip: { trigger: "axis", axisPointer: { type: "shadow" } },
    grid: GRID,
    xAxis: { type: "value", minInterval: 1 },
    yAxis: { type: "category", inverse: true, data: counts.map((entry) => entry.register) },
    series: [
      {
        type: "bar",
        name: "Events",
        label: { show: true, position: "right" },
        data: counts.map((entry) => ({ value: entry.count, itemStyle: { color: colours.get(entry.register) } })),
      },
    ],
  };
}
