import { currentTokens } from "@joinedcontext/sdk";
import { shortDays } from "./i18n";
import type { Lang } from "./i18n";

/**
 * The hours of the week as an ECharts heat map: Monday at the top, hour 0 to 23 across, each cell
 * the alerts that start then. `null` when no alert has a time, so the card says so instead.
 */
export function weekOption(matrix: number[][], lang: Lang): Record<string, unknown> | null {
  const cells: [number, number, number][] = [];
  let most = 0;
  matrix.forEach((row, day) =>
    row.forEach((count, hour) => {
      // ECharts' y axis grows upwards; Monday is drawn at the top.
      cells.push([hour, 6 - day, count]);
      most = Math.max(most, count);
    }),
  );
  if (most === 0) return null;
  const tokens = currentTokens();
  const days = [...shortDays(lang)].reverse();
  return {
    grid: { left: 40, right: 12, top: 8, bottom: 56 },
    tooltip: { position: "top" },
    xAxis: { type: "category", data: Array.from({ length: 24 }, (_, hour) => String(hour)), splitArea: { show: true } },
    yAxis: { type: "category", data: days, splitArea: { show: true } },
    visualMap: {
      min: 0,
      max: most,
      calculable: false,
      orient: "horizontal",
      left: "center",
      bottom: 0,
      inRange: { color: [tokens.color.surface, tokens.map.low, tokens.map.high] },
    },
    series: [{ type: "heatmap", data: cells, emphasis: { itemStyle: { borderColor: tokens.map.selected, borderWidth: 2 } } }],
  };
}

/** The cell a click names, back as a weekday (Monday 0) and an hour, or `null`. */
export function cellOf(params: unknown): { weekday: number; hour: number } | null {
  const value = (params as { value?: unknown }).value;
  if (!Array.isArray(value) || typeof value[0] !== "number" || typeof value[1] !== "number") return null;
  return { weekday: 6 - value[1], hour: value[0] };
}
