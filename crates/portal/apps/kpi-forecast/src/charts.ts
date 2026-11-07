import type { SeriesResult } from "./analysis";
import { dayShort, moment, t, value } from "./i18n";
import type { Lang } from "./i18n";
import { seriesColours } from "./theme";

interface Hovered {
  seriesName?: string;
  marker?: string;
  value?: unknown;
}

/**
 * The tooltip of a moment: its time on Helsinki's clock, whatever the reader's own zone, then each
 * line's value; the band's two halves are left out. Every text is the App's own words and numbers.
 */
export function tooltipOf(params: unknown, lang: Lang): string {
  const hovered = (Array.isArray(params) ? params : [params]) as Hovered[];
  const at = hovered.find((h) => Array.isArray(h.value))?.value as [number, number] | undefined;
  if (!at) return "";
  const lines = hovered
    .filter((h) => h.seriesName !== t(lang, "band") && Array.isArray(h.value) && typeof (h.value as unknown[])[1] === "number")
    .map((h) => `${h.marker ?? ""}${h.seriesName ?? ""}: ${value(lang, (h.value as [number, number])[1])}`);
  return [moment(lang, at[0]), ...lines].join("<br/>");
}

/**
 * One indicator over time as an ECharts option: what was measured, what the model expected one
 * step ahead, the forecast with its 95 % band, and the points that look wrong. `null` when the
 * indicator has no history, so the card says so instead.
 */
export function detailOption(result: SeriesResult, lang: Lang): Record<string, unknown> | null {
  if (result.history.length === 0) return null;
  const colours = seriesColours();
  const ahead = result.forecast;
  // The band is drawn as its floor and its height stacked on it, the height filled.
  const floor = ahead.map((a) => [a.t, a.lo]);
  const height = ahead.map((a) => [a.t, a.hi - a.lo]);
  return {
    grid: { left: 56, right: 16, top: 40, bottom: 32 },
    legend: { top: 0, data: [t(lang, "measured"), t(lang, "model"), t(lang, "forecast"), t(lang, "band"), t(lang, "anomaly")] },
    tooltip: { trigger: "axis", formatter: (params: unknown) => tooltipOf(params, lang) },
    xAxis: { type: "time", axisLabel: { hideOverlap: true, formatter: (ms: number) => dayShort(lang, ms) } },
    yAxis: { type: "value", scale: true, axisLabel: { formatter: (v: number) => value(lang, v) } },
    series: [
      {
        name: t(lang, "band"),
        type: "line",
        data: floor,
        stack: "band",
        symbol: "none",
        lineStyle: { opacity: 0 },
        itemStyle: { color: colours.forecast },
        tooltip: { show: false },
      },
      {
        name: t(lang, "band"),
        type: "line",
        data: height,
        stack: "band",
        symbol: "none",
        lineStyle: { opacity: 0 },
        areaStyle: { color: colours.forecast, opacity: 0.2 },
        itemStyle: { color: colours.forecast },
        tooltip: { show: false },
      },
      { name: t(lang, "measured"), type: "line", data: result.history, symbol: "none", lineStyle: { color: colours.history, width: 2 }, itemStyle: { color: colours.history } },
      {
        name: t(lang, "model"),
        type: "line",
        data: result.fitted,
        symbol: "none",
        lineStyle: { color: colours.fitted, width: 1, type: "dashed" },
        itemStyle: { color: colours.fitted },
      },
      {
        name: t(lang, "forecast"),
        type: "line",
        data: ahead.map((a) => [a.t, a.v]),
        symbol: "none",
        lineStyle: { color: colours.forecast, width: 2, type: "dashed" },
        itemStyle: { color: colours.forecast },
      },
      {
        name: t(lang, "anomaly"),
        type: "scatter",
        data: result.anomalies.map((a) => ({ value: [a.t, a.v], name: moment(lang, a.t) })),
        symbolSize: 10,
        itemStyle: { color: colours.anomaly },
      },
    ],
  };
}
