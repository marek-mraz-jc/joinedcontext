/**
 * The Vega-Lite specs the dashboard page draws its charts with (the Vega the viz renderer of
 * components/viz runs, ADR-N-053), one builder per Grafana panel type. Pure functions of the
 * frames and the panel's saved options: data goes in inline, nothing is fetched by Vega.
 */
import type { Frame, Series } from "./frames";
import { displayNameOf, numberOf, reducedOf, rowsOf, seriesOf, timeOf } from "./frames";
import type { FieldConfigSource, Thresholds } from "./fieldConfig";
import { colorOf, displayValue, settingsFor, stepFor } from "./fieldConfig";
import { seriesColor } from "./palette";

export type Spec = Record<string, unknown>;

/** The colours of the page around a chart, read from the theme when it is drawn. */
export interface ChartTheme {
  text: string;
  muted: string;
  grid: string;
  surface: string;
  font: string;
}

/** Shared look: no Vega legend (the page draws its own), the theme's text and grid lines. */
export function configOf(theme: ChartTheme): Spec {
  return {
    background: null,
    view: { stroke: null },
    font: theme.font,
    axis: {
      labelColor: theme.muted,
      titleColor: theme.muted,
      gridColor: theme.grid,
      domainColor: theme.grid,
      tickColor: theme.grid,
      labelFontSize: 11,
      titleFontSize: 11,
      titleFontWeight: "normal",
    },
    legend: { disable: true },
  };
}

/**
 * The formatter Vega's axis labels call by key (`jcFormat(datum.value, key)`): each chart puts its
 * own unit and decimals under a key, so labels read `250 ms` and `42%`.
 */
const formatters = new Map<string, (value: number) => string>();

export function formatterKey(unit: string | undefined, decimals: number | undefined, locale: string): string {
  const key = `${unit ?? ""}|${decimals ?? ""}|${locale}`;
  if (!formatters.has(key)) {
    formatters.set(key, (value) => displayValue({ unit, decimals, mappings: [], custom: {} }, value, locale).text);
  }
  return key;
}

export function formatWith(value: unknown, key: string): string {
  const format = formatters.get(key);
  const number = numberOf(value);
  return format && number !== null ? format(number) : String(value ?? "");
}

const quoted = (text: string): string => `'${text.replace(/\\/g, "\\\\").replace(/'/g, "\\'")}'`;

/** A y axis whose labels are in the field's unit. */
function valueAxis(key: string, title?: string): Spec {
  return { title: title ?? null, labelExpr: `jcFormat(datum.value, ${quoted(key)})`, tickCount: 5 };
}

/** A time axis over the asked range. */
function timeAxis(): Spec {
  return { title: null, labelOverlap: true, labelFlush: true, tickCount: 6, grid: false };
}

/** Rules at each threshold step, when the panel draws its thresholds (`custom.thresholdsStyle`). */
function thresholdLayers(thresholds: Thresholds | undefined, mode: unknown): Spec[] {
  if (!thresholds || typeof mode !== "string" || mode === "off") return [];
  return thresholds.steps
    .filter((step) => step.value !== null)
    .map((step) => ({
      data: { values: [{ y: step.value }] },
      mark: {
        type: "rule",
        color: colorOf(step.color),
        strokeDash: mode.startsWith("dashed") ? [4, 4] : undefined,
        strokeWidth: 1,
      },
      encoding: { y: { field: "y", type: "quantitative" } },
    }));
}

export interface TimeSeriesOptions {
  fieldConfig?: FieldConfigSource;
  options?: Record<string, unknown>;
  from: number;
  to: number;
  locale: string;
  theme: ChartTheme;
  height: number;
  /** `timeseries`, or `barchart` drawn over time. */
  hidden?: ReadonlySet<string>;
}

/** The colour of each series: an override's fixed colour, else the palette's next. */
export function seriesColors(series: Series[], fieldConfig?: FieldConfigSource): string[] {
  return series.map((one, index) => {
    const settings = settingsFor(fieldConfig, one.field);
    if (settings.color?.mode === "fixed" && settings.color.fixedColor) return colorOf(settings.color.fixedColor);
    return seriesColor(index);
  });
}

/** Grafana's `custom.lineInterpolation` as Vega's interpolate. */
function interpolation(custom: Record<string, unknown>): string {
  switch (custom.lineInterpolation) {
    case "smooth":
      return "monotone";
    case "stepBefore":
      return "step-before";
    case "stepAfter":
      return "step-after";
    default:
      return "linear";
  }
}

/** A `timeseries` panel: one line (or bar, or point) per series, a hover rule naming each value. */
export function timeSeriesSpec(frames: Frame[], opts: TimeSeriesOptions): { spec: Spec; series: Series[]; colors: string[]; formatKey: string } {
  const series = seriesOf(frames);
  const colors = seriesColors(series, opts.fieldConfig);
  const first = series[0] ? settingsFor(opts.fieldConfig, series[0].field) : settingsFor(opts.fieldConfig, { name: "", type: "number", values: [] });
  const custom = first.custom;
  const formatKey = formatterKey(first.unit, first.decimals, opts.locale);
  const shown = series.filter((one) => !opts.hidden?.has(one.name));
  const values = shown.flatMap((one) => one.points.map((point) => ({ t: point.t, s: one.name, v: point.v })));
  const drawStyle = custom.drawStyle === "bars" || custom.drawStyle === "points" ? custom.drawStyle : "line";
  const pointCount = Math.max(0, ...shown.map((one) => one.points.length));
  const showPoints = custom.showPoints === "always" || (custom.showPoints !== "never" && drawStyle === "line" && pointCount <= 30);
  const fillOpacity = typeof custom.fillOpacity === "number" ? custom.fillOpacity / 100 : 0;
  const stacked = (custom.stacking as { mode?: string } | undefined)?.mode === "normal";
  const domain = shown.map((one) => one.name);
  const range = shown.map((one) => colors[series.indexOf(one)]);
  const color = { field: "s", type: "nominal", scale: { domain, range }, legend: null };
  const x = { field: "t", type: "temporal", scale: { domain: [opts.from, opts.to] }, axis: timeAxis() };
  const y: Spec = {
    field: "v",
    type: "quantitative",
    axis: valueAxis(formatKey, typeof custom.axisLabel === "string" && custom.axisLabel ? custom.axisLabel : undefined),
    scale: {
      zero: drawStyle === "bars" || fillOpacity > 0 || stacked,
      ...(first.min !== undefined ? { domainMin: first.min } : {}),
      ...(first.max !== undefined ? { domainMax: first.max } : {}),
    },
    ...(stacked ? { stack: "zero" } : { stack: null }),
  };
  const marks: Spec[] = [];
  if (drawStyle === "bars") {
    marks.push({ mark: { type: "bar", opacity: 0.85 }, encoding: { x, y, color } });
  } else if (drawStyle === "points") {
    marks.push({ mark: { type: "point", filled: true, size: 24 }, encoding: { x, y, color } });
  } else {
    if (fillOpacity > 0 || stacked) {
      marks.push({
        mark: { type: "area", opacity: Math.max(fillOpacity, stacked ? 0.35 : 0), interpolate: interpolation(custom) },
        encoding: { x, y, color },
      });
    }
    marks.push({
      mark: {
        type: "line",
        interpolate: interpolation(custom),
        strokeWidth: typeof custom.lineWidth === "number" ? Math.max(custom.lineWidth, 1) : 1.5,
        point: showPoints ? { filled: true, size: typeof custom.pointSize === "number" ? custom.pointSize * 6 : 20 } : false,
      },
      encoding: { x, y: stacked ? { ...y, stack: "zero" } : y, color },
    });
  }
  const tooltipMode = (opts.options?.tooltip as { mode?: string } | undefined)?.mode;
  if (tooltipMode !== "hidden" && tooltipMode !== "none" && values.length > 0) {
    // The hover rule: one row per time with every series' value, the pivot Vega-Lite documents.
    marks.push({
      transform: [{ pivot: "s", value: "v", groupby: ["t"] }],
      mark: { type: "rule", strokeWidth: 8, color: opts.theme.muted },
      encoding: {
        x: { field: "t", type: "temporal" },
        opacity: { condition: { value: 0.35, param: "hover", empty: false }, value: 0 },
        tooltip: [{ field: "t", type: "temporal" }, ...domain.map((name) => ({ field: name, type: "quantitative" }))],
      },
      params: [{ name: "hover", select: { type: "point", fields: ["t"], nearest: true, on: "pointerover", clear: "pointerout" } }],
    });
  }
  marks.push(...thresholdLayers(first.thresholds, (custom.thresholdsStyle as { mode?: string } | undefined)?.mode));
  return {
    spec: {
      $schema: "https://vega.github.io/schema/vega-lite/v6.json",
      width: "container",
      height: opts.height,
      autosize: { type: "fit", contains: "padding" },
      data: { values },
      layer: marks,
      config: configOf(opts.theme),
    },
    series,
    colors,
    formatKey,
  };
}

export interface CategoryOptions {
  fieldConfig?: FieldConfigSource;
  options?: Record<string, unknown>;
  locale: string;
  theme: ChartTheme;
  height: number;
}

/**
 * A `barchart` panel: the frame's string field (or `options.xField`) along one axis, one bar per
 * numeric field beside each other.
 */
export function barChartSpec(frames: Frame[], opts: CategoryOptions): { spec: Spec; names: string[]; colors: string[] } {
  const { columns, names, rows } = rowsOf(frames);
  const xField = typeof opts.options?.xField === "string" && names.includes(opts.options.xField)
    ? opts.options.xField
    : names[columns.findIndex((field) => field.type === "string" || field.type === "time")] ?? names[0];
  const valueNames = names.filter((name, index) => name !== xField && columns[index].type === "number");
  const values = rows.flatMap((row) =>
    valueNames.map((name) => ({
      x: columns[names.indexOf(xField)]?.type === "time" ? timeOf(row[xField]) : String(row[xField] ?? ""),
      s: name,
      v: numberOf(row[name]),
    })),
  );
  const firstValue = columns[names.indexOf(valueNames[0])];
  const settings = firstValue ? settingsFor(opts.fieldConfig, firstValue) : settingsFor(opts.fieldConfig, { name: "", type: "number", values: [] });
  const formatKey = formatterKey(settings.unit, settings.decimals, opts.locale);
  const colors = valueNames.map((_, index) => seriesColor(index));
  const horizontal = opts.options?.orientation === "horizontal";
  const category = { field: "x", type: columns[names.indexOf(xField)]?.type === "time" ? "ordinal" : "nominal", sort: null, axis: { title: null, labelLimit: 120 } };
  const value = { field: "v", type: "quantitative", axis: valueAxis(formatKey) };
  const offset = valueNames.length > 1 ? { field: "s" } : undefined;
  return {
    spec: {
      $schema: "https://vega.github.io/schema/vega-lite/v6.json",
      width: "container",
      height: opts.height,
      autosize: { type: "fit", contains: "padding" },
      data: { values },
      mark: { type: "bar", cornerRadiusEnd: 2, tooltip: true },
      encoding: {
        ...(horizontal ? { y: category, x: value, yOffset: offset } : { x: category, y: value, xOffset: offset }),
        color: { field: "s", type: "nominal", scale: { domain: valueNames, range: colors }, legend: null },
        tooltip: [
          { field: "x", title: xField },
          { field: "s", title: "series" },
          { field: "v", title: "value" },
        ],
      },
      config: configOf(opts.theme),
    },
    names: valueNames,
    colors,
  };
}

/** A `piechart` panel: one slice per reduced value, a donut when `pieType` says so. */
export function pieChartSpec(frames: Frame[], opts: CategoryOptions): { spec: Spec; slices: { name: string; value: number; color: string }[] } {
  const reduceOptions = (opts.options?.reduceOptions ?? {}) as { calcs?: string[]; values?: boolean };
  const reduced = reducedOf(frames, reduceOptions.calcs?.[0], reduceOptions.values === true);
  const slices = reduced
    .filter((one) => one.value !== null && one.value > 0)
    .map((one, index) => ({ name: one.name, value: one.value as number, color: seriesColor(index) }));
  const donut = opts.options?.pieType === "donut";
  const radius = Math.max(40, Math.floor(opts.height / 2) - 8);
  return {
    spec: {
      $schema: "https://vega.github.io/schema/vega-lite/v6.json",
      width: "container",
      height: opts.height,
      autosize: { type: "fit", contains: "padding" },
      data: { values: slices },
      mark: { type: "arc", innerRadius: donut ? Math.round(radius * 0.55) : 0, outerRadius: radius, stroke: opts.theme.surface, strokeWidth: 1, tooltip: true },
      encoding: {
        theta: { field: "value", type: "quantitative", stack: true },
        color: { field: "name", type: "nominal", scale: { domain: slices.map((s) => s.name), range: slices.map((s) => s.color) }, legend: null },
        tooltip: [
          { field: "name", title: "name" },
          { field: "value", title: "value" },
        ],
      },
      view: { stroke: null },
      config: configOf(opts.theme),
    },
    slices,
  };
}

/** The time a reading lasts: until the next one of the same series, the last until `to`. */
function spans(points: { t: number; v: unknown }[], to: number): { t: number; t2: number; v: unknown }[] {
  return points.map((point, index) => ({ t: point.t, t2: points[index + 1]?.t ?? Math.max(to, point.t + 1), v: point.v }));
}

export interface TimeOptions extends CategoryOptions {
  from: number;
  to: number;
}

/**
 * A `heatmap` panel: Prometheus buckets (fields labelled `le`) or each numeric field as a row,
 * each reading a cell from its time to the next, coloured by its value.
 */
export function heatmapSpec(frames: Frame[], opts: TimeOptions): { spec: Spec; buckets: string[] } {
  const series = seriesOf(frames);
  const bucketOf = (one: Series): string => one.field.labels?.le ?? one.name;
  const sorted = [...series].sort((a, b) => (Number(bucketOf(a)) || 0) - (Number(bucketOf(b)) || 0));
  const buckets = sorted.map(bucketOf);
  const values = sorted.flatMap((one) => spans(one.points, opts.to).map((span) => ({ ...span, b: bucketOf(one) })));
  const scheme = ((opts.options?.color as { scheme?: string } | undefined)?.scheme ?? "Oranges").toLowerCase();
  const settings = series[0] ? settingsFor(opts.fieldConfig, series[0].field) : settingsFor(opts.fieldConfig, { name: "", type: "number", values: [] });
  const formatKey = formatterKey(settings.unit, settings.decimals, opts.locale);
  return {
    spec: {
      $schema: "https://vega.github.io/schema/vega-lite/v6.json",
      width: "container",
      height: opts.height,
      autosize: { type: "fit", contains: "padding" },
      data: { values: values.filter((cell) => cell.v !== null) },
      mark: { type: "rect", tooltip: true },
      encoding: {
        x: { field: "t", type: "temporal", scale: { domain: [opts.from, opts.to] }, axis: timeAxis() },
        x2: { field: "t2" },
        y: { field: "b", type: "ordinal", sort: buckets.slice().reverse(), axis: { title: null } },
        color: { field: "v", type: "quantitative", scale: { scheme }, legend: { disable: false, title: null, orient: "right", labelExpr: `jcFormat(datum.value, ${quoted(formatKey)})` } },
        tooltip: [
          { field: "t", type: "temporal", title: "time" },
          { field: "b", title: "bucket" },
          { field: "v", title: "value" },
        ],
      },
      config: { ...configOf(opts.theme), legend: { labelColor: opts.theme.muted } },
    },
    buckets,
  };
}

/**
 * A `state-timeline` panel: one row per field, each state a bar from its time to the next,
 * coloured and named by the field's value mappings, else by its thresholds.
 */
export function stateTimelineSpec(frames: Frame[], opts: TimeOptions): { spec: Spec; states: { text: string; color: string }[] } {
  const rows: { t: number; t2: number; row: string; text: string; color: string }[] = [];
  const states = new Map<string, string>();
  let fallback = 0;
  for (const frame of frames) {
    const time = frame.fields.find((field) => field.type === "time");
    if (!time) continue;
    for (const field of frame.fields) {
      if (field === time) continue;
      const settings = settingsFor(opts.fieldConfig, field, frame.refId);
      const points = time.values
        .map((at, index) => ({ t: timeOf(at), v: field.values[index] }))
        .filter((point): point is { t: number; v: unknown } => point.t !== null)
        .sort((a, b) => a.t - b.t);
      for (const span of spans(points, opts.to)) {
        const shown = displayValue(settings, span.v, opts.locale);
        const step = typeof span.v === "number" ? stepFor(settings.thresholds, span.v, settings.min, settings.max) : undefined;
        let color = shown.color ?? (step ? colorOf(step.color) : undefined);
        if (!color) {
          color = states.get(shown.text) ?? seriesColor(fallback);
          if (!states.has(shown.text)) fallback += 1;
        }
        states.set(shown.text, color);
        rows.push({ t: span.t, t2: span.t2, row: displayNameOf(field, frame), text: shown.text, color });
      }
    }
  }
  return {
    spec: {
      $schema: "https://vega.github.io/schema/vega-lite/v6.json",
      width: "container",
      height: opts.height,
      autosize: { type: "fit", contains: "padding" },
      data: { values: rows },
      mark: { type: "bar", tooltip: true, height: { band: 0.8 } },
      encoding: {
        x: { field: "t", type: "temporal", scale: { domain: [opts.from, opts.to] }, axis: timeAxis() },
        x2: { field: "t2" },
        y: { field: "row", type: "nominal", axis: { title: null, labelLimit: 140 } },
        color: { field: "color", type: "nominal", scale: null, legend: null },
        tooltip: [
          { field: "row", title: "field" },
          { field: "text", title: "state" },
          { field: "t", type: "temporal", title: "from" },
          { field: "t2", type: "temporal", title: "to" },
        ],
      },
      config: configOf(opts.theme),
    },
    states: [...states.entries()].map(([text, color]) => ({ text, color })),
  };
}

/** A `histogram` panel: every numeric value of the frames in `options.bucketCount` bins. */
export function histogramSpec(frames: Frame[], opts: CategoryOptions): Spec {
  const values = frames.flatMap((frame) =>
    frame.fields.filter((field) => field.type === "number").flatMap((field) => field.values.map(numberOf).filter((v): v is number => v !== null).map((v) => ({ v }))),
  );
  const count = typeof opts.options?.bucketCount === "number" ? opts.options.bucketCount : 30;
  return {
    $schema: "https://vega.github.io/schema/vega-lite/v6.json",
    width: "container",
    height: opts.height,
    autosize: { type: "fit", contains: "padding" },
    data: { values },
    mark: { type: "bar", color: seriesColor(0), tooltip: true },
    encoding: {
      x: { field: "v", bin: { maxbins: count }, type: "quantitative", axis: { title: null } },
      y: { aggregate: "count", type: "quantitative", axis: { title: null } },
    },
    config: configOf(opts.theme),
  };
}

/**
 * One `gauge`: a 240° track, the threshold bands as a thin outer ring and the value's arc in its
 * step's colour, the value printed in the middle.
 */
export function gaugeSpec(
  value: number | null,
  text: string,
  settings: { min?: number; max?: number; thresholds?: Thresholds },
  opts: { theme: ChartTheme; height: number; showThresholdMarkers?: boolean },
): Spec {
  const min = settings.min ?? 0;
  const max = settings.max ?? (settings.thresholds?.mode === "percentage" ? 100 : Math.max(100, value ?? 0));
  const start = (-2 * Math.PI) / 3;
  const sweep = (4 * Math.PI) / 3;
  const angle = (v: number): number => start + sweep * Math.min(1, Math.max(0, (v - min) / (max - min || 1)));
  const step = stepFor(settings.thresholds, value, min, max);
  const radius = Math.max(30, Math.floor(opts.height / 2) - 6);
  const steps = settings.thresholds?.steps ?? [];
  const bands = steps.map((one, index) => {
    const lower = one.value === null ? min : settings.thresholds?.mode === "percentage" ? min + ((max - min) * one.value) / 100 : one.value;
    const next = steps[index + 1];
    const upper = next ? (settings.thresholds?.mode === "percentage" ? min + ((max - min) * (next.value ?? 0)) / 100 : (next.value ?? max)) : max;
    return { a: angle(lower), b: angle(upper), color: colorOf(one.color), r: radius, r2: radius - 4 };
  });
  const arcs = [
    { a: start, b: start + sweep, color: opts.theme.grid, r: radius - 7, r2: radius * 0.62 },
    ...(value !== null ? [{ a: start, b: angle(value), color: colorOf(step?.color ?? "green"), r: radius - 7, r2: radius * 0.62 }] : []),
    ...(opts.showThresholdMarkers === false ? [] : bands),
  ];
  return {
    $schema: "https://vega.github.io/schema/vega-lite/v6.json",
    width: "container",
    height: opts.height,
    autosize: { type: "fit", contains: "padding" },
    layer: [
      {
        data: { values: arcs },
        mark: { type: "arc", cornerRadius: 1 },
        encoding: {
          theta: { field: "a", type: "quantitative", scale: null },
          theta2: { field: "b" },
          radius: { field: "r", type: "quantitative", scale: null },
          radius2: { field: "r2" },
          color: { field: "color", type: "nominal", scale: null, legend: null },
        },
      },
      {
        data: { values: [{ text }] },
        mark: { type: "text", fontSize: Math.max(14, Math.round(radius / 3)), fontWeight: 600, color: opts.theme.text, baseline: "middle", dy: radius * 0.1 },
        encoding: { text: { field: "text" } },
      },
    ],
    config: configOf(opts.theme),
  };
}
