/**
 * The categorical and time-span charts of a dashboard, each a Vega-Lite spec of `specs.ts`:
 * `barchart`, `piechart`, `heatmap`, `state-timeline` and `histogram`, with a key of what each
 * colour means beside the ones whose colours carry it.
 */
import { clsx } from "clsx";
import { useMemo } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { Frame } from "./frames";
import type { FieldConfigSource } from "./fieldConfig";
import { barChartSpec, heatmapSpec, histogramSpec, pieChartSpec, stateTimelineSpec } from "./specs";
import { chartTheme, VegaChart } from "./VegaChart";
import { Swatch } from "./paint";

export interface ChartPanelProps {
  title: string;
  frames: Frame[];
  fieldConfig?: FieldConfigSource;
  options?: Record<string, unknown>;
  height: number;
  locale: string;
  from: number;
  to: number;
}

/** What each colour stands for, as swatches with their names. */
export function ColourKey({ items, label, className }: { items: { name: string; color: string; detail?: string }[]; label: string; className?: string }): JSX.Element | null {
  if (items.length === 0) return null;
  return (
    <ul aria-label={label} className={clsx("flex flex-wrap gap-x-4 gap-y-1 text-caption", className)}>
      {items.map((item) => (
        <li key={item.name} className="flex min-w-0 items-center gap-1.5">
          <Swatch color={item.color} shape="square" />
          <span className="truncate">{item.name}</span>
          {item.detail ? <span className="tabular-nums text-fg-muted">{item.detail}</span> : null}
        </li>
      ))}
    </ul>
  );
}

/** The datum under the pointer as label and value lines, its own field names as labels. */
function datumLines(datum: Record<string, unknown>, keys: [string, string][], dateTime: Intl.DateTimeFormat): JSX.Element {
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
      {keys
        .filter(([key]) => datum[key] !== undefined && datum[key] !== null)
        .map(([key, label]) => {
          const value = datum[key];
          const text = (key === "t" || key === "t2") && typeof value === "number" ? dateTime.format(value) : String(value);
          return [
            <dt key={`${key}-k`} className="text-fg-muted">
              {label}
            </dt>,
            <dd key={`${key}-v`} className="tabular-nums">
              {text}
            </dd>,
          ];
        })}
    </dl>
  );
}

function useDateTime(locale: string): Intl.DateTimeFormat {
  return useMemo(() => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }), [locale]);
}

export function BarChartPanel({ title, frames, fieldConfig, options, height, locale }: ChartPanelProps): JSX.Element {
  const { t } = useTranslation();
  const built = useMemo(
    () => barChartSpec(frames, { fieldConfig, options, locale, theme: chartTheme(), height: Math.max(120, height - 28) }),
    [frames, fieldConfig, options, locale, height],
  );
  const dateTime = useDateTime(locale);
  return (
    <div className="flex flex-col gap-2">
      <VegaChart
        spec={built.spec}
        height={Math.max(120, height - 28)}
        label={t("dashboardView.chartLabel", { title, n: built.names.length })}
        tooltip={(datum) => datumLines(datum, [["x", t("dashboardView.category")], ["s", t("dashboardView.series")], ["v", t("dashboardView.value")]], dateTime)}
      />
      {built.names.length > 1 ? (
        <ColourKey label={t("dashboardView.legend")} items={built.names.map((name, index) => ({ name, color: built.colors[index] }))} />
      ) : null}
    </div>
  );
}

export function PieChartPanel({ title, frames, fieldConfig, options, height, locale }: ChartPanelProps): JSX.Element {
  const { t } = useTranslation();
  const chartHeight = Math.max(140, height - 36);
  const built = useMemo(
    () => pieChartSpec(frames, { fieldConfig, options, locale, theme: chartTheme(), height: chartHeight }),
    [frames, fieldConfig, options, locale, chartHeight],
  );
  const total = built.slices.reduce((sum, slice) => sum + slice.value, 0);
  const percent = new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 1 });
  const dateTime = useDateTime(locale);
  return (
    <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
      <VegaChart
        className="min-w-0 sm:flex-1"
        spec={built.spec}
        height={chartHeight}
        label={t("dashboardView.chartLabel", { title, n: built.slices.length })}
        tooltip={(datum) => datumLines(datum, [["name", t("dashboardView.series")], ["value", t("dashboardView.value")]], dateTime)}
      />
      {/* Beside the pie the key keeps to half the width at most, or the pie gets none. */}
      <ColourKey
        className="sm:max-w-1/2 sm:flex-col sm:flex-nowrap"
        label={t("dashboardView.legend")}
        items={built.slices.map((slice) => ({ name: slice.name, color: slice.color, detail: percent.format(slice.value / (total || 1)) }))}
      />
    </div>
  );
}

export function HeatmapPanel({ title, frames, fieldConfig, options, height, locale, from, to }: ChartPanelProps): JSX.Element {
  const { t } = useTranslation();
  const built = useMemo(
    () =>
      heatmapSpec(frames, {
        fieldConfig,
        options,
        locale,
        theme: chartTheme(),
        height,
        from,
        to,
        // Prometheus buckets (`le`) count readings up to each bound.
        titles: frames.some((frame) => frame.fields.some((field) => field.labels?.le !== undefined)) ? { bucket: t("dashboardView.upperBound"), value: t("dashboardView.count") } : undefined,
      }),
    [frames, fieldConfig, options, locale, height, from, to, t],
  );
  const dateTime = useDateTime(locale);
  return (
    <VegaChart
      spec={built.spec}
      height={height}
      label={t("dashboardView.chartLabel", { title, n: built.buckets.length })}
      tooltip={(datum) => datumLines(datum, [["t", t("dashboardView.time")], ["b", t("dashboardView.bucket")], ["v", t("dashboardView.value")]], dateTime)}
    />
  );
}

export function StateTimelinePanel({ title, frames, fieldConfig, options, height, locale, from, to }: ChartPanelProps): JSX.Element {
  const { t } = useTranslation();
  const chartHeight = Math.max(100, height - 28);
  const built = useMemo(
    () => stateTimelineSpec(frames, { fieldConfig, options, locale, theme: chartTheme(), height: chartHeight, from, to }),
    [frames, fieldConfig, options, locale, chartHeight, from, to],
  );
  const dateTime = useDateTime(locale);
  return (
    <div className="flex flex-col gap-2">
      <VegaChart
        spec={built.spec}
        height={chartHeight}
        label={t("dashboardView.chartLabel", { title, n: built.states.length })}
        tooltip={(datum) =>
          datumLines(datum, [["row", t("dashboardView.series")], ["text", t("dashboardView.state")], ["t", t("dashboardView.since")], ["t2", t("dashboardView.until")]], dateTime)
        }
      />
      <ColourKey label={t("dashboardView.legend")} items={built.states.map((state) => ({ name: state.text, color: state.color }))} />
    </div>
  );
}

export function HistogramPanel({ title, frames, fieldConfig, options, height, locale }: ChartPanelProps): JSX.Element {
  const { t } = useTranslation();
  const spec = useMemo(() => histogramSpec(frames, { fieldConfig, options, locale, theme: chartTheme(), height }), [frames, fieldConfig, options, locale, height]);
  return <VegaChart spec={spec} height={height} label={t("dashboardView.chartLabel", { title, n: 1 })} />;
}
