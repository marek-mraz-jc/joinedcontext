import { currentTokens } from "@joinedcontext/sdk";
import type { DesignTokens, Row } from "@joinedcontext/sdk";
import { colorRamp } from "./components/EntityMap";
import { BANDS, histogram, ranked } from "./stations";

const GRID = { containLabel: true, left: 8, right: 24, top: 24, bottom: 8 };
/** `containLabel` makes room for the axes' labels only: a bar's own value label ("100 %"),
 * drawn right of a full bar, needs the margin itself or is cut (T-3127). */
const RANKED_GRID = { ...GRID, right: 52 };

/** Stations by bikes available now, one bar per band, coloured along the map's ramp (T-2924). */
export function histogramOption(rows: Row[], tokens?: DesignTokens): Record<string, unknown> {
  const t = tokens ?? currentTokens();
  const last = BANDS.length - 1;
  return {
    color: t.chart.palette,
    tooltip: { trigger: "axis" },
    grid: GRID,
    xAxis: { type: "category", name: "Bikes available", nameLocation: "middle", nameGap: 28, data: BANDS.map((band) => band.label) },
    yAxis: { type: "value", name: "Stations", minInterval: 1 },
    series: [
      {
        type: "bar",
        name: "Stations",
        data: histogram(rows).map((value, index) => ({ value, itemStyle: { color: colorRamp(index, [0, last], t) } })),
      },
    ],
  };
}

/**
 * The `n` fullest or emptiest stations as horizontal bars of their share of docks with a bike, the
 * first one on top; `null` when no station has both counts, so the card says there is nothing to chart.
 */
export function rankedOption(rows: Row[], order: "fullest" | "emptiest", n = 10, tokens?: DesignTokens): Record<string, unknown> | null {
  const t = tokens ?? currentTokens();
  const top = ranked(rows, n, order);
  if (top.length === 0) return null;
  return {
    color: t.chart.palette,
    tooltip: { trigger: "axis", axisPointer: { type: "shadow" } },
    grid: RANKED_GRID,
    xAxis: { type: "value", min: 0, max: 100, axisLabel: { formatter: "{value} %" } },
    yAxis: { type: "category", inverse: true, data: top.map((station) => station.name) },
    series: [
      {
        type: "bar",
        name: "Docks with a bike (%)",
        label: { show: true, position: "right", formatter: "{c} %" },
        data: top.map((station) => ({ value: station.percent, itemStyle: { color: colorRamp(station.percent, [0, 100], t) } })),
      },
    ],
  };
}
