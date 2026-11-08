/**
 * The panels that show one value per field: `stat` (the value large, coloured by its threshold,
 * with a sparkline), `gauge` (Vega's arc) and `bargauge` (one bar per field, as long as its share
 * of min to max). Each reduces the frames as `options.reduceOptions` says.
 */
import { useMemo } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import type { Frame, Reduced } from "./frames";
import { reducedOf } from "./frames";
import type { FieldConfigSource, FieldSettings } from "./fieldConfig";
import { colorOf, displayValue, settingsFor, stepFor } from "./fieldConfig";
import { gaugeSpec } from "./specs";
import { chartTheme, VegaChart } from "./VegaChart";
import { Fill, Tinted } from "./paint";

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

/** Each field's reduced value, its text, its threshold colour and its share of the range. */
export function shownValues(frames: Frame[], fieldConfig: FieldConfigSource | undefined, options: Record<string, unknown> | undefined, locale: string): Shown[] {
  const reduceOptions = (options?.reduceOptions ?? {}) as ReduceOptions;
  const reduced = reducedOf(frames, reduceOptions.calcs?.[0] ?? "lastNotNull", reduceOptions.values === true);
  const limited = reduceOptions.limit ? reduced.slice(0, reduceOptions.limit) : reduced;
  const values = limited.map((one) => one.value).filter((value): value is number => value !== null);
  return limited.map((reduced) => {
    const settings = settingsFor(fieldConfig, reduced.field);
    const min = settings.min ?? Math.min(0, ...values);
    const max = settings.max ?? Math.max(...values, min + 1);
    const shown = displayValue(settings, reduced.value, locale);
    const step = stepFor(settings.thresholds, reduced.value, min, max);
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
            className={clsx("flex h-full min-w-0 flex-col justify-center rounded-md p-3", !background && "bg-surface-subtle")}
          >
            {showName ? <span className={clsx("truncate text-caption", !background && "text-fg-muted")}>{one.reduced.name}</span> : null}
            <span className="truncate text-display font-semibold tabular-nums">{textMode === "name" ? one.reduced.name : one.text}</span>
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

/** A `gauge` panel: one arc per value, side by side. */
export function GaugePanel({ title, frames, fieldConfig, options, height, locale }: ValuePanelProps): JSX.Element {
  const { t } = useTranslation();
  const shown = useMemo(() => shownValues(frames, fieldConfig, options, locale), [frames, fieldConfig, options, locale]);
  const theme = useMemo(() => chartTheme(), []);
  const each = Math.max(120, height - 28);
  return (
    <ul aria-label={title} className="grid h-full grid-cols-[repeat(auto-fit,minmax(10rem,1fr))] gap-2">
      {shown.map((one) => (
        <li key={one.reduced.name} className="flex min-w-0 flex-col items-center">
          <VegaChart
            spec={gaugeSpec(one.reduced.value, one.text, { min: one.min, max: one.max, thresholds: one.settings.thresholds }, {
              theme,
              height: each,
              showThresholdMarkers: options?.showThresholdMarkers !== false,
            })}
            height={each}
            label={t("dashboardView.gaugeLabel", { name: one.reduced.name, value: one.text })}
          />
          {shown.length > 1 || options?.textMode === "value_and_name" ? <p className="truncate text-caption text-fg-muted">{one.reduced.name}</p> : null}
        </li>
      ))}
    </ul>
  );
}

/** A `bargauge` panel: one bar per value, its length the value's share of min to max. */
export function BarGaugePanel({ title, frames, fieldConfig, options, locale }: ValuePanelProps): JSX.Element {
  const shown = useMemo(() => shownValues(frames, fieldConfig, options, locale), [frames, fieldConfig, options, locale]);
  const vertical = options?.orientation === "vertical";
  if (vertical) {
    return (
      <ul aria-label={title} className="flex h-full items-end gap-3 overflow-x-auto">
        {shown.map((one) => (
          <li key={one.reduced.name} className="flex h-full min-w-12 flex-1 flex-col items-center justify-end gap-1">
            <span className="text-caption font-medium tabular-nums">{one.text}</span>
            <span className="flex w-full flex-1 items-end rounded-sm bg-surface-muted">
              <Fill share={one.share} color={one.color} vertical />
            </span>
            <span className="max-w-full truncate text-caption text-fg-muted">{one.reduced.name}</span>
          </li>
        ))}
      </ul>
    );
  }
  return (
    <ul aria-label={title} className="flex flex-col gap-2">
      {shown.map((one) => (
        <li key={one.reduced.name} className="grid grid-cols-[minmax(0,10rem)_1fr_auto] items-center gap-2 max-sm:grid-cols-[1fr_auto]">
          <span className="truncate text-caption text-fg-muted max-sm:col-span-2">{one.reduced.name}</span>
          <span className="h-3 overflow-hidden rounded-sm bg-surface-muted" role="meter" aria-label={one.reduced.name} aria-valuemin={one.min} aria-valuemax={one.max} aria-valuenow={one.reduced.value ?? undefined} aria-valuetext={one.text}>
            <Fill share={one.share} color={one.color} />
          </span>
          <span className="text-caption font-medium tabular-nums">{one.text}</span>
        </li>
      ))}
    </ul>
  );
}
