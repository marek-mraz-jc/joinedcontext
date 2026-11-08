/**
 * A Grafana `timeseries` panel: Vega's lines over the asked range, the panel's own legend (a list,
 * or a table of its `calcs`) and a tooltip naming every series at the time under the pointer.
 */
import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { Button, Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "../ui";
import type { Frame, Series } from "./frames";
import { reduce } from "./frames";
import type { FieldConfigSource } from "./fieldConfig";
import { displayValue, settingsFor } from "./fieldConfig";
import { formatWith, timeSeriesSpec } from "./specs";
import { chartTheme, VegaChart } from "./VegaChart";
import { Swatch } from "./paint";

export interface LegendOptions {
  displayMode?: string;
  placement?: string;
  showLegend?: boolean;
  calcs?: string[];
  sortBy?: string;
  sortDesc?: boolean;
}

/** The words of Grafana's reducers, as a legend's column heads read them. */
const CALC_KEYS: Record<string, string> = {
  lastNotNull: "last",
  last: "last",
  max: "max",
  min: "min",
  mean: "mean",
  sum: "sum",
  first: "first",
  firstNotNull: "first",
  count: "count",
  range: "range",
  delta: "delta",
};

export interface LegendProps {
  series: Series[];
  colors: string[];
  fieldConfig?: FieldConfigSource;
  legend: LegendOptions;
  locale: string;
  hidden: ReadonlySet<string>;
  onToggle: (name: string) => void;
}

/** A series' swatch, its name and its calcs; a click shows or hides it on the chart. */
export function Legend({ series, colors, fieldConfig, legend, locale, hidden, onToggle }: LegendProps): JSX.Element | null {
  const { t } = useTranslation();
  if (legend.showLegend === false || legend.displayMode === "hidden" || series.length === 0) return null;
  const calcs = (legend.calcs ?? []).filter((calc) => calc in CALC_KEYS);
  const rows = series.map((one, index) => {
    const settings = settingsFor(fieldConfig, one.field);
    const values = one.points.map((point) => point.v);
    return {
      one,
      color: colors[index],
      calcs: calcs.map((calc) => {
        const value = reduce(values, calc);
        return { calc, value, text: value === null ? "–" : displayValue(settings, value, locale).text };
      }),
    };
  });
  if (legend.sortBy) {
    const at = calcs.findIndex((calc) => t(`dashboardView.calc.${CALC_KEYS[calc]}`) === legend.sortBy || legend.sortBy?.toLowerCase().startsWith(CALC_KEYS[calc]));
    if (at >= 0) {
      rows.sort((a, b) => ((a.calcs[at].value ?? -Infinity) - (b.calcs[at].value ?? -Infinity)) * (legend.sortDesc ? -1 : 1));
    }
  }
  const toggle = (name: string, off: boolean) => (
    <Button
      variant="ghost"
      size="xs"
      aria-pressed={!off}
      onClick={() => onToggle(name)}
      className={clsx("min-w-0 max-w-full justify-start", off && "text-fg-subtle line-through")}
    >
      <Swatch color={rows.find((row) => row.one.name === name)?.color ?? ""} faded={off} />
      <span className="truncate">{name}</span>
    </Button>
  );
  if (legend.displayMode === "table" && calcs.length > 0) {
    return (
      <Table caption={t("dashboardView.legend")} zebra={false} maxHeight="max-h-48" className="text-caption">
        <TableHead>
          <TableHeaderCell>{t("dashboardView.series")}</TableHeaderCell>
          {calcs.map((calc) => (
            <TableHeaderCell key={calc} align="right">
              {t(`dashboardView.calc.${CALC_KEYS[calc]}`)}
            </TableHeaderCell>
          ))}
        </TableHead>
        <TableBody>
          {rows.map((row) => (
            <TableRow key={row.one.name}>
              <TableCell>{toggle(row.one.name, hidden.has(row.one.name))}</TableCell>
              {row.calcs.map((calc) => (
                <TableCell key={calc.calc} align="right" className="tabular-nums">
                  {calc.text}
                </TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
      </Table>
    );
  }
  return (
    <ul aria-label={t("dashboardView.legend")} className="flex flex-wrap gap-x-4 gap-y-1">
      {rows.map((row) => (
        <li key={row.one.name} className="flex min-w-0 items-center gap-2">
          {toggle(row.one.name, hidden.has(row.one.name))}
          {row.calcs.map((calc) => (
            <span key={calc.calc} className="text-caption tabular-nums text-fg-muted">
              {t(`dashboardView.calc.${CALC_KEYS[calc.calc]}`)} {calc.text}
            </span>
          ))}
        </li>
      ))}
    </ul>
  );
}

export interface TimeSeriesPanelProps {
  title: string;
  frames: Frame[];
  fieldConfig?: FieldConfigSource;
  options?: Record<string, unknown>;
  from: number;
  to: number;
  height: number;
  locale: string;
}

export function TimeSeriesPanel({ title, frames, fieldConfig, options, from, to, height, locale }: TimeSeriesPanelProps): JSX.Element {
  const { t } = useTranslation();
  const [hidden, setHidden] = useState<ReadonlySet<string>>(new Set());
  const legend = (options?.legend ?? {}) as LegendOptions;
  const right = legend.placement === "right";
  const chartHeight = Math.max(120, right ? height : height - (legend.displayMode === "table" ? 96 : 36));
  const built = useMemo(
    () => timeSeriesSpec(frames, { fieldConfig, options, from, to, locale, theme: chartTheme(), height: chartHeight, hidden }),
    [frames, fieldConfig, options, from, to, locale, chartHeight, hidden],
  );
  const dateTime = useMemo(
    () => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }),
    [locale],
  );
  const tooltip = (datum: Record<string, unknown>) => {
    const at = Number(datum.t);
    const shown = built.series
      .map((one, index) => ({ one, color: built.colors[index], value: datum[one.name] }))
      .filter((row) => row.value !== undefined && row.value !== null);
    const sort = (options?.tooltip as { sort?: string } | undefined)?.sort;
    if (sort === "desc" || sort === "asc") {
      shown.sort((a, b) => (Number(a.value) - Number(b.value)) * (sort === "desc" ? -1 : 1));
    }
    return (
      <div className="flex flex-col gap-1">
        <p className="font-medium">{Number.isFinite(at) ? dateTime.format(at) : ""}</p>
        <ul className="flex flex-col gap-0.5">
          {shown.slice(0, 12).map((row) => (
            <li key={row.one.name} className="flex items-center gap-2">
              <Swatch color={row.color} />
              <span className="min-w-0 flex-1 truncate">{row.one.name}</span>
              <span className="tabular-nums">{displayValue(settingsFor(fieldConfig, row.one.field), row.value, locale).text || formatWith(row.value, built.formatKey)}</span>
            </li>
          ))}
        </ul>
      </div>
    );
  };
  return (
    <div className={clsx("flex h-full min-w-0 gap-3", right ? "flex-col sm:flex-row" : "flex-col")}>
      <VegaChart
        className="min-w-0 flex-1"
        spec={built.spec}
        height={chartHeight}
        label={t("dashboardView.chartLabel", { title, count: built.series.length })}
        tooltip={tooltip}
      />
      <div className={clsx("min-w-0", right && "sm:w-2/5 sm:shrink-0")}>
        <Legend
          series={built.series}
          colors={built.colors}
          fieldConfig={fieldConfig}
          legend={legend}
          locale={locale}
          hidden={hidden}
          onToggle={(name) =>
            setHidden((current) => {
              const next = new Set(current);
              if (next.has(name)) next.delete(name);
              else next.add(name);
              return next;
            })
          }
        />
      </div>
    </div>
  );
}
