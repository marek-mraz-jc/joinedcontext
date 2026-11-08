/**
 * The panels that show one value per field: `stat` (the value large, coloured by its threshold,
 * with a sparkline), `gauge` (Vega's arc) and `bargauge` (one bar per field, as long as its share
 * of min to max). Each reduces the frames as `options.reduceOptions` says.
 */
import { useCallback, useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import type { Frame, Reduced } from "./frames";
import { reducedOf } from "./frames";
import type { FieldConfigSource, FieldSettings, Thresholds } from "./fieldConfig";
import { colorOf, displayValue, settingsFor, stepFor } from "./fieldConfig";
import { gaugeSpec } from "./specs";
import { chartTheme, VegaChart } from "./VegaChart";
import { Fill, Swatch, Tinted } from "./paint";

interface ReduceOptions {
  calcs?: string[];
  values?: boolean;
  limit?: number;
}

export interface ValuePanelProps {
  title: string;
  frames: Frame[];
  fieldConfig?: FieldConfigSource;
  options?: Record<string, unknown>;
  height: number;
  locale: string;
}

interface Shown {
  reduced: Reduced;
  settings: FieldSettings;
  text: string;
  color: string;
  /** The value's share of min to max, 0 to 1. */
  share: number;
  min: number;
  max: number;
}

/** Grafana's thresholds for a value panel whose dashboard saves none: green, red from 80. */
export const DEFAULT_THRESHOLDS: Thresholds = { mode: "absolute", steps: [{ color: "green", value: null }, { color: "red", value: 80 }] };

/**
 * The lowest and highest of every number in the frames, every row of every numeric field: the
 * range Grafana scales a stat, gauge or bar gauge to when the field sets no min or max.
 */
export function globalRange(frames: Frame[]): { min: number; max: number } | undefined {
  let [min, max] = [Infinity, -Infinity];
  for (const field of frames.flatMap((frame) => frame.fields)) {
    if (field.type !== "number") continue;
    for (const value of field.values) {
      if (typeof value !== "number" || !Number.isFinite(value)) continue;
      min = Math.min(min, value);
      max = Math.max(max, value);
    }
  }
  return min <= max ? { min, max } : undefined;
}

/** Each field's reduced value, its text, its threshold colour and its share of the range. */
export function shownValues(frames: Frame[], fieldConfig: FieldConfigSource | undefined, options: Record<string, unknown> | undefined, locale: string): Shown[] {
  const reduceOptions = (options?.reduceOptions ?? {}) as ReduceOptions;
  const reduced = reducedOf(frames, reduceOptions.calcs?.[0] ?? "lastNotNull", reduceOptions.values === true);
  const limited = reduceOptions.limit ? reduced.slice(0, reduceOptions.limit) : reduced;
  const range = globalRange(frames) ?? { min: 0, max: 100 };
  return limited.map((reduced) => {
    const settings = settingsFor(fieldConfig, reduced.field);
    const min = settings.min ?? range.min;
    const max = settings.max ?? (range.max > min ? range.max : min + 1);
    const shown = displayValue(settings, reduced.value, locale);
    const step = settings.color?.mode === "fixed" ? undefined : stepFor(settings.thresholds ?? DEFAULT_THRESHOLDS, reduced.value, min, max);
    const name = settings.displayName ?? reduced.name;
    return {
      reduced: { ...reduced, name },
      settings,
      text: shown.text,
      color: shown.color ?? colorOf(step?.color ?? (settings.color?.fixedColor ?? "green")),
      share: reduced.value === null ? 0 : Math.min(1, Math.max(0, (reduced.value - min) / (max - min || 1))),
      min,
      max,
    };
  });
}

/**
 * What each colour of a gauge or bar means: a swatch per threshold step and the value it starts
 * at, in the field's unit (a percentage step in percent of min to max).
 */
export function ThresholdKey({ settings, locale }: { settings: FieldSettings; locale: string }): JSX.Element | null {
  const { t } = useTranslation();
  const steps = settings.thresholds?.steps ?? [];
  if (steps.length < 2) return null;
  const percent = settings.thresholds?.mode === "percentage";
  const at = (value: number): string => (percent ? `${value}%` : displayValue({ ...settings, mappings: [] }, value, locale).text);
  return (
    <ul aria-label={t("dashboardView.thresholds")} className="flex flex-wrap justify-center gap-x-3 gap-y-1 text-caption text-fg-muted">
      {steps.map((step, index) => (
        <li key={index} className="flex items-center gap-1.5">
          <Swatch color={colorOf(step.color)} shape="square" />
          {step.value === null ? t("dashboardView.below", { value: at(steps[1].value ?? 0) }) : t("dashboardView.from", { value: at(step.value) })}
        </li>
      ))}
    </ul>
  );
}

/** A `stat` panel: each value large, in its threshold's colour, a sparkline under it. */
export function StatPanel({ title, frames, fieldConfig, options, height, locale }: ValuePanelProps): JSX.Element {
  const { t } = useTranslation();
  const shown = useMemo(() => shownValues(frames, fieldConfig, options, locale), [frames, fieldConfig, options, locale]);
  const background = options?.colorMode === "background";
  const textMode = typeof options?.textMode === "string" ? options.textMode : "auto";
  const showName = textMode === "value_and_name" || textMode === "name" || (textMode === "auto" && shown.length > 1);
  const graph = options?.graphMode !== "none";
  return (
    <ul aria-label={title} className={clsx("grid h-full gap-2", shown.length > 1 ? "grid-cols-[repeat(auto-fit,minmax(8rem,1fr))]" : "grid-cols-1")}>
      {shown.map((one) => (
        <li key={one.reduced.name} className="min-w-0">
          <Tinted
            color={background || options?.colorMode !== "none" ? one.color : undefined}
            background={background}
            className="flex h-full min-w-0 flex-col items-center justify-center rounded-md p-3 text-center"
          >
            {showName ? <span className={clsx("max-w-full truncate text-caption", !background && "text-fg-muted")}>{one.reduced.name}</span> : null}
            {/* Grafana centres the value in the panel, in its largest text. */}
            <span className="max-w-full truncate text-display font-semibold tabular-nums">{textMode === "name" ? one.reduced.name : one.text}</span>
            {graph && one.reduced.trend.length > 1 ? (
              <Sparkline points={one.reduced.trend} color="currentColor" label={t("dashboardView.trend", { name: one.reduced.name })} height={Math.max(24, Math.min(64, height / 4))} />
            ) : null}
          </Tinted>
        </li>
      ))}
    </ul>
  );
}

/** The line of a value over time, as an SVG polyline: a stat's background trend. */
export function Sparkline({ points, color, label, height }: { points: { t: number; v: number | null }[]; color: string; label: string; height: number }): JSX.Element | null {
  const present = points.filter((point): point is { t: number; v: number } => point.v !== null);
  if (present.length < 2) return null;
  const [t0, t1] = [present[0].t, present[present.length - 1].t];
  const values = present.map((point) => point.v);
  const [lo, hi] = [Math.min(...values), Math.max(...values)];
  const line = present
    .map((point) => `${(((point.t - t0) / (t1 - t0 || 1)) * 100).toFixed(2)},${(40 - ((point.v - lo) / (hi - lo || 1)) * 38 - 1).toFixed(2)}`)
    .join(" ");
  return (
    <svg role="img" aria-label={label} viewBox="0 0 100 40" preserveAspectRatio="none" className="mt-1 w-full" height={height}>
      <polyline points={line} fill="none" stroke={color} strokeWidth="1.5" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

/** A `gauge` panel: one arc per value, side by side, its min and max at the arc's ends. */
export function GaugePanel({ title, frames, fieldConfig, options, height, locale }: ValuePanelProps): JSX.Element {
  const { t } = useTranslation();
  const shown = useMemo(() => shownValues(frames, fieldConfig, options, locale), [frames, fieldConfig, options, locale]);
  const theme = useMemo(() => chartTheme(), []);
  const named = shown.length > 1 || options?.textMode === "value_and_name";
  const markers = options?.showThresholdMarkers !== false;
  const each = Math.max(120, height - (named ? 20 : 0) - (markers ? 24 : 0));
  return (
    <div className="flex h-full flex-col gap-1">
      <ul aria-label={title} className="grid min-h-0 flex-1 grid-cols-[repeat(auto-fit,minmax(10rem,1fr))] gap-2">
        {shown.map((one) => (
          <li key={one.reduced.name} className="flex min-w-0 flex-col items-center">
            <VegaChart
              className="w-full"
              spec={gaugeSpec(one.reduced.value, one.text, { min: one.min, max: one.max, thresholds: one.settings.thresholds }, {
                theme,
                height: each,
                showThresholdMarkers: markers,
                minText: displayValue({ ...one.settings, mappings: [] }, one.min, locale).text,
                maxText: displayValue({ ...one.settings, mappings: [] }, one.max, locale).text,
              })}
              height={each}
              label={t("dashboardView.gaugeLabel", { name: one.reduced.name, value: one.text })}
            />
            {named ? <p className="truncate text-caption text-fg-muted">{one.reduced.name}</p> : null}
          </li>
        ))}
      </ul>
      {markers && shown[0] ? <ThresholdKey settings={shown[0].settings} locale={locale} /> : null}
    </div>
  );
}

/**
 * The CSS gradient of a bar gauge in `gradient` mode, as Grafana's BarGauge draws it: the base
 * colour up to halfway between the previous threshold and the next one the value passed, then
 * blended into that threshold's colour. Stops sit at their place on the whole scale, so a short bar shows only
 * its first colours; `share` (0 to 1) is the filled bar's length on that scale.
 */
export function barGradient(thresholds: Thresholds, value: number, min: number, max: number, share: number, vertical: boolean): string {
  const percent = thresholds.mode === "percentage";
  const along = (at: number): number => Math.min(1, Math.max(0, (at - min) / (max - min || 1)));
  const stops: string[] = [];
  let last = 0;
  for (const step of thresholds.steps) {
    const color = colorOf(step.color);
    if (stops.length === 0) {
      stops.push(color, color);
      continue;
    }
    const place = step.value === null ? 0 : percent ? Math.min(1, Math.max(0, step.value / 100)) : along(step.value);
    const starts = step.value === null ? -Infinity : percent ? min + (max - min) * place : step.value;
    if (value < starts) break;
    const offset = place - (place - last) / 2;
    last = place;
    // Grafana appends the blend's start to the colour before it, and the new colour ends the bar.
    stops[stops.length - 1] += ` ${share > 0 ? ((offset / share) * 100).toFixed(1) : "0"}%`;
    stops.push(color);
  }
  return `linear-gradient(${vertical ? "to top" : "to right"}, ${stops.join(", ")})`;
}

/** The panel's width, read once it is laid out and again when it changes. */
function useWidth(): [(element: HTMLElement | null) => (() => void) | undefined, number] {
  const [width, setWidth] = useState(0);
  const ref = useCallback((element: HTMLElement | null) => {
    if (!element) return;
    setWidth(element.clientWidth);
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => setWidth(element.clientWidth));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, width];
}

/**
 * A `bargauge` panel: one bar per value, its length the value's share of min to max, the value in
 * its threshold's colour. As Grafana's: `orientation` auto stands the bars up when the panel is
 * wider than tall, and `displayMode` gradient (the default) blends the threshold colours the
 * value passed, `basic` and `lcd` fill in the value's colour.
 */
export function BarGaugePanel({ title, frames, fieldConfig, options, height, locale }: ValuePanelProps): JSX.Element {
  const shown = useMemo(() => shownValues(frames, fieldConfig, options, locale), [frames, fieldConfig, options, locale]);
  const [measure, width] = useWidth();
  const vertical = options?.orientation === "vertical" || ((options?.orientation ?? "auto") === "auto" && width > height);
  const gradient = (options?.displayMode ?? "gradient") === "gradient";
  const paint = (one: Shown): string | undefined =>
    gradient && one.reduced.value !== null ? barGradient(one.settings.thresholds ?? DEFAULT_THRESHOLDS, one.reduced.value, one.min, one.max, one.share, vertical) : undefined;
  const meter = (one: Shown) => ({
    role: "meter",
    "aria-label": one.reduced.name,
    "aria-valuemin": one.min,
    "aria-valuemax": one.max,
    "aria-valuenow": one.reduced.value ?? undefined,
    "aria-valuetext": one.text,
  });
  // A key only for thresholds the dashboard saved; Grafana's defaults need none.
  const key = shown[0]?.settings.thresholds ? <ThresholdKey settings={shown[0].settings} locale={locale} /> : null;
  if (vertical) {
    return (
      <div ref={measure} className="flex h-full flex-col gap-2">
        <ul aria-label={title} className="flex min-h-0 flex-1 items-stretch gap-2 overflow-x-auto">
          {shown.map((one) => (
            <li key={one.reduced.name} className="flex min-w-12 flex-1 flex-col items-center gap-1">
              <Tinted color={one.color} className="text-title font-medium tabular-nums">
                {one.text}
              </Tinted>
              <span className="flex w-full flex-1 items-end overflow-hidden rounded-sm bg-surface-muted" {...meter(one)}>
                <Fill share={one.share} color={one.color} paint={paint(one)} vertical />
              </span>
              <span className="max-w-full truncate text-caption">{one.reduced.name}</span>
            </li>
          ))}
        </ul>
        {key}
      </div>
    );
  }
  // The bars share the panel's height, as Grafana's do: thicker when there are few of them.
  const room = height / Math.max(1, shown.length);
  const thickness = room >= 64 ? "h-8" : room >= 40 ? "h-5" : "h-3";
  return (
    <div ref={measure} className="flex h-full flex-col gap-2">
      <ul aria-label={title} className="flex min-h-0 flex-1 flex-col justify-around gap-2">
        {shown.map((one) => (
          <li key={one.reduced.name} className="grid grid-cols-[minmax(0,10rem)_1fr_auto] items-center gap-2 max-sm:grid-cols-[1fr_auto]">
            <span className="truncate text-caption max-sm:col-span-2">{one.reduced.name}</span>
            <span className={clsx(thickness, "overflow-hidden rounded-sm bg-surface-muted")} {...meter(one)}>
              <Fill share={one.share} color={one.color} paint={paint(one)} />
            </span>
            <Tinted color={one.color} className="text-body font-medium tabular-nums">
              {one.text}
            </Tinted>
          </li>
        ))}
      </ul>
      {key}
    </div>
  );
}
