/**
 * The colours a saved dashboard names (Grafana's named colours, a hex or an rgb() text) and the
 * series palette, as concrete values: Vega draws on a canvas, which reads no CSS variable. The
 * named colours are data the dashboard carries, so they read the same in both schemes; the series
 * palette is the chart theme's.
 */
import theme from "../../../../../shared/jc-chart/src/theme.json";

/** The chart theme's two configs (T-6886): crates/shared/jc-chart/src/theme.json, the one file every chart surface reads. */
export type ChartMode = "light" | "dark";
export const THEME: Record<ChartMode, typeof theme.light> = theme;

/** The page's scheme as the Portal's tokens follow it (tokens.css: `prefers-color-scheme`). */
export function chartMode(): ChartMode {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

/** Grafana's named colours, as the Portal's palette draws them. */
const NAMED: Record<string, string> = {
  green: "#15803d",
  "dark-green": "#166534",
  "semi-dark-green": "#16a34a",
  "light-green": "#4ade80",
  "super-light-green": "#86efac",
  red: "#dc2626",
  "dark-red": "#991b1b",
  "semi-dark-red": "#b91c1c",
  "light-red": "#f87171",
  "super-light-red": "#fca5a5",
  yellow: "#ca8a04",
  "dark-yellow": "#a16207",
  "semi-dark-yellow": "#b45309",
  "light-yellow": "#facc15",
  "super-light-yellow": "#fde047",
  orange: "#ea580c",
  "dark-orange": "#c2410c",
  "semi-dark-orange": "#ea580c",
  "light-orange": "#fb923c",
  "super-light-orange": "#fdba74",
  blue: "#2563eb",
  "dark-blue": "#1e40af",
  "semi-dark-blue": "#1d4ed8",
  "light-blue": "#60a5fa",
  "super-light-blue": "#93c5fd",
  purple: "#7c3aed",
  "dark-purple": "#5b21b6",
  "semi-dark-purple": "#6d28d9",
  "light-purple": "#a78bfa",
  "super-light-purple": "#c4b5fd",
  text: "#475569",
  transparent: "transparent",
};

/** A colour a dashboard names, as CSS: a Grafana name, a hex or an rgb() text. Anything else is grey. */
export function colorOf(name: string | undefined): string {
  if (!name) return NAMED.text;
  const named = NAMED[name];
  if (named) return named;
  return /^#[0-9a-f]{3,8}$/i.test(name) || /^rgba?\([\d\s.,%]+\)$/i.test(name) ? name : NAMED.text;
}

/** On a phone a band axis's name takes at most a third of the width, as in the viz frame (T-6865); nothing elsewhere. */
export function narrowConfig(): Record<string, unknown> {
  return typeof window !== "undefined" && window.innerWidth < 480 ? { axisY: { labelLimit: Math.round(window.innerWidth / 3) } } : {};
}

/** The colour of the `index`-th series: the theme's `range.category` for the page's scheme, round. */
export function seriesColor(index: number, mode: ChartMode = chartMode()): string {
  const palette = THEME[mode].range.category;
  return palette[index % palette.length] ?? THEME[mode].mark.color;
}
