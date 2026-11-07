import type { ReactNode } from "react";
import { useEffect, useId, useMemo, useRef } from "react";
import * as echarts from "echarts";
import { aggregate, currentTokens, echartsTheme, groupBy } from "@joinedcontext/sdk";
import type { Agg, DesignTokens, ProblemError, Row, TemporalRow } from "@joinedcontext/sdk";
import { Empty, Loading, Problem } from "./states";
import * as i18n from "../i18n";

export interface ChartSpec {
  x: string;
  y?: string;
  agg?: Agg;
  top?: number;
}

export type Bucket = "hour" | "day" | "week" | "month";

export function bucketOf(iso: string, bucket: Bucket): string | null {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;

  const y = d.getUTCFullYear();
  const m = String(d.getUTCMonth() + 1).padStart(2, "0");
  const dt = String(d.getUTCDate()).padStart(2, "0");
  const h = String(d.getUTCHours()).padStart(2, "0");

  switch (bucket) {
    case "hour":
      return `${y}-${m}-${dt}T${h}:00:00.000Z`;
    case "day":
      return `${y}-${m}-${dt}T00:00:00.000Z`;
    case "month":
      return `${y}-${m}-01T00:00:00.000Z`;
    case "week": {
      const day = d.getUTCDay();
      const diff = (day === 0 ? -6 : 1) - day;
      const monday = new Date(Date.UTC(y, d.getUTCMonth(), d.getUTCDate() + diff, 0, 0, 0, 0));
      return monday.toISOString();
    }
  }
}

function hasSeriesData(option: Record<string, unknown>): boolean {
  const series = option.series as Array<{ data?: unknown[] }> | undefined;
  if (!Array.isArray(series) || series.length === 0) return false;
  return series.some((s) => Array.isArray(s.data) && s.data.length > 0);
}

export function barOption(
  rows: Row[],
  spec: ChartSpec & { horizontal?: boolean },
  tokens?: DesignTokens,
): Record<string, unknown> {
  const t = tokens ?? currentTokens();
  const agg = spec.agg ?? (spec.y ? "sum" : "count");
  const groups = groupBy(rows, spec.x, agg, spec.y, spec.top ?? 20);

  const categoryAxis = { type: "category", data: groups.map((g) => g.key) };
  const valueAxis = { type: "value" };
  const series = [{ type: "bar", name: spec.y ?? i18n.t("chart.count"), data: groups.map((g) => g.value) }];

  return {
    color: t.chart.palette,
    tooltip: { trigger: "axis" },
    grid: { containLabel: true, left: 8, right: 16, top: 16, bottom: 8 },
    xAxis: spec.horizontal ? valueAxis : categoryAxis,
    yAxis: spec.horizontal ? categoryAxis : valueAxis,
    series,
  };
}

export function lineOption(
  rows: Row[],
  spec: ChartSpec,
  tokens?: DesignTokens,
): Record<string, unknown> {
  const t = tokens ?? currentTokens();
  const agg = spec.agg ?? (spec.y ? "sum" : "count");
  const groups = groupBy(rows, spec.x, agg, spec.y);

  const allNumeric =
    groups.length > 0 &&
    groups.every((g) => g.key.trim() !== "" && !Number.isNaN(Number(g.key)));

  const sorted = [...groups].sort((a, b) => {
    if (allNumeric) {
      return Number(a.key) - Number(b.key);
    }
    return a.key.localeCompare(b.key);
  });

  return {
    color: t.chart.palette,
    tooltip: { trigger: "axis" },
    grid: { containLabel: true, left: 8, right: 16, top: 16, bottom: 8 },
    xAxis: { type: "category", data: sorted.map((g) => g.key) },
    yAxis: { type: "value" },
    series: [{ type: "line", showSymbol: false, data: sorted.map((g) => g.value) }],
  };
}

export function pieOption(
  rows: Row[],
  spec: ChartSpec,
  tokens?: DesignTokens,
): Record<string, unknown> {
  const t = tokens ?? currentTokens();
  const agg = spec.agg ?? (spec.y ? "sum" : "count");
  const groups = groupBy(rows, spec.x, agg, spec.y);

  const limit = spec.top ?? 8;
  let data: Array<{ name: string; value: number }>;

  if (groups.length <= limit) {
    data = groups.map((g) => ({ name: g.key, value: g.value }));
  } else {
    const topGroups = groups.slice(0, limit);
    const rest = groups.slice(limit);
    const otherVal = rest.reduce((acc, g) => acc + g.value, 0);
    data = [...topGroups.map((g) => ({ name: g.key, value: g.value })), { name: i18n.t("chart.other"), value: otherVal }];
  }

  return {
    color: t.chart.palette,
    tooltip: { trigger: "item" },
    series: [{ type: "pie", radius: ["40%", "70%"], data }],
  };
}

export function timeSeriesOption(
  input:
    | { rows: Row[]; time: string; y?: string; agg?: Agg; bucket?: Bucket }
    | { series: TemporalRow[]; attr: string },
  tokens?: DesignTokens,
): Record<string, unknown> {
  const t = tokens ?? currentTokens();

  if ("rows" in input) {
    const { rows, time, y, agg = y ? "avg" : "count", bucket = "day" } = input;
    const buckets = new Map<string, Row[]>();
    for (const row of rows) {
      const rawTime = row[time];
      if (typeof rawTime !== "string") continue;
      const b = bucketOf(rawTime, bucket);
      if (!b) continue;
      const existing = buckets.get(b);
      if (existing) existing.push(row);
      else buckets.set(b, [row]);
    }
    const points: Array<[string, number]> = [];
    for (const [b, bucketRows] of buckets.entries()) {
      const val = aggregate(bucketRows, agg, y);
      if (val !== null) {
        points.push([b, val]);
      }
    }
    points.sort((a, b) => a[0].localeCompare(b[0]));

    return {
      color: t.chart.palette,
      tooltip: { trigger: "axis" },
      grid: { containLabel: true, left: 8, right: 16, top: 16, bottom: 8 },
      xAxis: { type: "time" },
      yAxis: { type: "value" },
      series: [{ type: "line", data: points }],
    };
  }

  const { series, attr } = input;
  const lineSeries = series.map((s) => {
    const pts = (s.series[attr] ?? [])
      .filter((p) => typeof p.value === "number")
      .map((p) => [p.observedAt, p.value as number] as [string, number])
      .sort((a, b) => a[0].localeCompare(b[0]));
    return {
      type: "line",
      // The local id, not the whole URN: a legend of URNs is one entry per line on a phone.
      name: s.id.split(":").pop() || s.id,
      data: pts,
    };
  });
  const legend = lineSeries.length > 1;

  return {
    color: t.chart.palette,
    tooltip: { trigger: "axis" },
    // Scrolls rather than wraps, so a narrow chart keeps its plot area.
    ...(legend ? { legend: { type: "scroll", bottom: 0 } } : {}),
    grid: { containLabel: true, left: 8, right: 16, top: 16, bottom: legend ? 36 : 8 },
    xAxis: { type: "time" },
    yAxis: { type: "value" },
    series: lineSeries,
  };
}

/** What a chart shows, in rows: the first column names the category, slice or instant, and each
 * series has a column of its own. */
export interface ChartTable {
  head: string[];
  rows: string[][];
}

type Series = { type?: unknown; name?: unknown; data?: unknown };

/**
 * The values a chart draws, as a table a screen reader reads (WCAG 1.1.1, T-2974), from the same
 * option the canvas is drawn from, so the two never disagree. `null` when the option has nothing
 * a person could read off it.
 */
export function chartTable(option: Record<string, unknown>): ChartTable | null {
  const series = (Array.isArray(option.series) ? option.series : []) as Series[];
  if (series.length === 0) return null;
  const number = new Intl.NumberFormat(i18n.language(), { maximumFractionDigits: 2 });
  const cell = (value: unknown): string =>
    typeof value === "number" && Number.isFinite(value) ? number.format(value) : "";
  const named = (s: Series): string => (typeof s.name === "string" && s.name !== "" ? s.name : i18n.t("chart.value"));
  const data = (s: Series): unknown[] => (Array.isArray(s.data) ? s.data : []);

  if (series[0].type === "pie") {
    const slices = data(series[0]) as Array<{ name?: unknown; value?: unknown }>;
    const rows = slices.map((slice) => [String(slice.name ?? ""), cell(slice.value)]);
    return rows.length > 0 ? { head: [i18n.t("chart.category"), i18n.t("chart.value")], rows } : null;
  }

  const axes = [option.xAxis, option.yAxis] as Array<{ type?: unknown; data?: unknown } | undefined>;
  const categories = axes.find((axis) => axis?.type === "category" && Array.isArray(axis.data))?.data as unknown[] | undefined;
  if (categories) {
    const rows = categories.map((category, index) => [String(category), ...series.map((s) => cell(data(s)[index]))]);
    return rows.length > 0 ? { head: [i18n.t("chart.category"), ...series.map(named)], rows } : null;
  }

  if (axes.some((axis) => axis?.type === "time")) {
    // Each series carries its own instants; the table holds every instant once, in order.
    const byInstant = new Map<string, string[]>();
    series.forEach((s, column) => {
      for (const point of data(s)) {
        if (!Array.isArray(point) || typeof point[0] !== "string") continue;
        const row = byInstant.get(point[0]) ?? series.map(() => "");
        row[column] = cell(point[1]);
        byInstant.set(point[0], row);
      }
    });
    const when = new Intl.DateTimeFormat(i18n.language(), { dateStyle: "medium", timeStyle: "short" });
    const rows = [...byInstant.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([instant, values]) => {
        const date = new Date(instant);
        return [Number.isNaN(date.getTime()) ? instant : when.format(date), ...values];
      });
    return rows.length > 0 ? { head: [i18n.t("chart.time"), ...series.map(named)], rows } : null;
  }
  return null;
}

export function ChartCard({
  title,
  option,
  height,
  loading,
  error,
  empty,
  onSelect,
  onReady,
}: {
  title?: string;
  option: Record<string, unknown> | null;
  height?: number;
  loading?: boolean;
  error?: ProblemError | Error | null;
  empty?: ReactNode;
  onSelect?: (name: string) => void;
  onReady?: (chart: echarts.ECharts) => void;
}): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;
  const onReadyRef = useRef(onReady);
  onReadyRef.current = onReady;

  const showCanvas = !error && !loading && option !== null;
  const table = useMemo(() => (option ? chartTable(option) : null), [option]);
  // Named by its caption outright: not every screen reader, nor jsdom, names a figure from it.
  const captionId = useId();

  useEffect(() => {
    if (!showCanvas) return;
    const el = containerRef.current;
    if (!el) return;

    const chart = echarts.init(el, echartsTheme(currentTokens()), { renderer: "canvas" });
    chartRef.current = chart;
    onReadyRef.current?.(chart);

    chart.on("click", (p: unknown) => {
      const param = p as { name?: unknown };
      if (param && param.name !== undefined) {
        onSelectRef.current?.(String(param.name));
      }
    });

    if (option) {
      chart.setOption(option, true);
    }

    let observer: ResizeObserver | undefined;
    if (typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(() => {
        chart.resize();
      });
      observer.observe(el);
    }

    return () => {
      observer?.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, [showCanvas]);

  useEffect(() => {
    if (chartRef.current && option) {
      chartRef.current.setOption(option, true);
    }
  }, [option]);

  return (
    <figure className="jc-chart" aria-labelledby={title ? captionId : undefined}>
      {title && <figcaption id={captionId}>{title}</figcaption>}
      {error ? (
        <Problem error={error} />
      ) : loading ? (
        <Loading />
      ) : option === null ? (
        <Empty>{empty ?? i18n.t("chart.empty")}</Empty>
      ) : (
        <>
          {/* The canvas is a picture of the table under it, which is what a screen reader reads. */}
          <div
            className="jc-chart-canvas"
            aria-hidden="true"
            style={height === undefined ? undefined : { height }}
            ref={containerRef}
          />
          {table && (
            <table className="jc-chart-table">
              <thead>
                <tr>
                  {table.head.map((name, index) => (
                    <th key={index} scope="col">
                      {name}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {table.rows.map(([label, ...values], index) => (
                  <tr key={index}>
                    <th scope="row">{label}</th>
                    {values.map((value, column) => (
                      <td key={column}>{value}</td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </figure>
  );
}

export function BarChartCard({
  rows,
  x,
  y,
  agg,
  top,
  horizontal,
  title,
  height,
  loading,
  error,
  onSelect,
}: ChartSpec & {
  rows: Row[];
  title?: string;
  horizontal?: boolean;
  height?: number;
  loading?: boolean;
  error?: ProblemError | Error | null;
  onSelect?: (name: string) => void;
}): React.JSX.Element {
  const option = useMemo(() => {
    const opt = barOption(rows, { x, y, agg, top, horizontal });
    return hasSeriesData(opt) ? opt : null;
  }, [rows, x, y, agg, top, horizontal]);

  return (
    <ChartCard
      title={title}
      option={option}
      height={height}
      loading={loading}
      error={error}
      onSelect={onSelect}
    />
  );
}

export function LineChartCard({
  rows,
  x,
  y,
  agg,
  top,
  title,
  height,
  loading,
  error,
}: ChartSpec & {
  rows: Row[];
  title?: string;
  height?: number;
  loading?: boolean;
  error?: ProblemError | Error | null;
}): React.JSX.Element {
  const option = useMemo(() => {
    const opt = lineOption(rows, { x, y, agg, top });
    return hasSeriesData(opt) ? opt : null;
  }, [rows, x, y, agg, top]);

  return (
    <ChartCard
      title={title}
      option={option}
      height={height}
      loading={loading}
      error={error}
    />
  );
}

export function PieChartCard({
  rows,
  x,
  y,
  agg,
  top,
  title,
  height,
  loading,
  error,
  onSelect,
}: ChartSpec & {
  rows: Row[];
  title?: string;
  height?: number;
  loading?: boolean;
  error?: ProblemError | Error | null;
  onSelect?: (name: string) => void;
}): React.JSX.Element {
  const option = useMemo(() => {
    const opt = pieOption(rows, { x, y, agg, top });
    return hasSeriesData(opt) ? opt : null;
  }, [rows, x, y, agg, top]);

  return (
    <ChartCard
      title={title}
      option={option}
      height={height}
      loading={loading}
      error={error}
      onSelect={onSelect}
    />
  );
}

export function TimeSeriesCard(
  props: ({ rows: Row[]; time: string; y?: string; agg?: Agg; bucket?: Bucket } | { series: TemporalRow[]; attr: string }) & {
    title?: string;
    height?: number;
    loading?: boolean;
    error?: ProblemError | Error | null;
  },
): React.JSX.Element {
  const { title, height, loading, error } = props;
  const option = useMemo(() => {
    const opt = timeSeriesOption(props);
    return hasSeriesData(opt) ? opt : null;
  }, [props]);

  return (
    <ChartCard
      title={title}
      option={option}
      height={height}
      loading={loading}
      error={error}
    />
  );
}
