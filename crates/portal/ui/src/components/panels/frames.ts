/**
 * daiya's data frames as the dashboard page reads them (crates/daiya/model/src/frames.rs): one
 * frame per query result, columns of `time`, `number`, `string`, `bool`, `json` or `geo` values.
 */

export type FieldKind = "time" | "number" | "string" | "bool" | "json" | "geo";

export interface FrameField {
  name: string;
  type: FieldKind;
  unit?: string;
  displayName?: string;
  labels?: Record<string, string>;
  values: unknown[];
}

export interface Frame {
  name?: string;
  refId?: string;
  fields: FrameField[];
}

/** One line of a time series: its name, its field and its points in time order. */
export interface Series {
  name: string;
  field: FrameField;
  points: { t: number; v: number | null }[];
}

/** A time as epoch milliseconds: a number, or an ISO 8601 text. */
export function timeOf(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const parsed = Date.parse(value);
    return Number.isNaN(parsed) ? null : parsed;
  }
  return null;
}

/** A number, or `null` for a gap; Prometheus' `NaN` and `±Inf` texts are gaps too. */
export function numberOf(value: unknown): number | null {
  if (typeof value === "number") return Number.isFinite(value) ? value : null;
  if (typeof value === "boolean") return value ? 1 : 0;
  if (typeof value === "string" && value.trim() !== "") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

/**
 * The name Grafana gives a field: its display name, else its labels (`job=edge, code=200`), else
 * its own name, prefixed by the frame's when a frame holds a single value field of a generic name.
 */
export function displayNameOf(field: FrameField, frame?: Frame): string {
  if (field.displayName) return field.displayName;
  const labels = Object.entries(field.labels ?? {}).filter(([key]) => key !== "__name__");
  if (labels.length === 1) return labels[0][1];
  if (labels.length > 1) return labels.map(([key, value]) => `${key}=${value}`).join(", ");
  if (frame?.name && (field.name === "Value" || field.name === "value")) return frame.name;
  return field.name;
}

/** Every numeric field of every frame against that frame's time field, as series. */
export function seriesOf(frames: Frame[]): Series[] {
  const out: Series[] = [];
  for (const frame of frames) {
    const time = frame.fields.find((field) => field.type === "time");
    if (!time) continue;
    for (const field of frame.fields) {
      if (field.type !== "number") continue;
      const points = time.values
        .map((at, index) => ({ t: timeOf(at), v: numberOf(field.values[index]) }))
        .filter((point): point is { t: number; v: number | null } => point.t !== null)
        .sort((a, b) => a.t - b.t);
      out.push({ name: displayNameOf(field, frame), field, points });
    }
  }
  return out;
}

/** The rows of the frames as records keyed by display name, for tables and categorical charts. */
export function rowsOf(frames: Frame[]): { columns: FrameField[]; names: string[]; rows: Record<string, unknown>[] } {
  const columns: FrameField[] = [];
  const names: string[] = [];
  const rows: Record<string, unknown>[] = [];
  for (const frame of frames) {
    const length = Math.max(0, ...frame.fields.map((field) => field.values.length));
    const local = frame.fields.map((field) => {
      let name = displayNameOf(field, frames.length > 1 ? frame : undefined);
      if (!names.includes(name)) {
        names.push(name);
        columns.push(field);
      } else if (columns[names.indexOf(name)] !== field && frames.length === 1) {
        name = `${name} ${names.filter((n) => n.startsWith(name)).length + 1}`;
        names.push(name);
        columns.push(field);
      }
      return name;
    });
    for (let index = 0; index < length; index += 1) {
      const row: Record<string, unknown> = {};
      frame.fields.forEach((field, at) => {
        row[local[at]] = field.values[index] ?? null;
      });
      rows.push(row);
    }
  }
  return { columns, names, rows };
}

/** Grafana's reducers a legend, a stat or a gauge names (`options.reduceOptions.calcs`). */
export type Reducer = "lastNotNull" | "last" | "first" | "firstNotNull" | "max" | "min" | "mean" | "sum" | "count" | "range" | "delta" | "diff";

export function reduce(values: (number | null)[], reducer: string): number | null {
  const present = values.filter((value): value is number => value !== null);
  switch (reducer) {
    case "last":
      return values.length ? values[values.length - 1] : null;
    case "first":
      return values.length ? values[0] : null;
    case "firstNotNull":
      return present.length ? present[0] : null;
    case "max":
      return present.length ? Math.max(...present) : null;
    case "min":
      return present.length ? Math.min(...present) : null;
    case "mean":
    case "avg":
      return present.length ? present.reduce((a, b) => a + b, 0) / present.length : null;
    case "sum":
      return present.length ? present.reduce((a, b) => a + b, 0) : null;
    case "count":
      return values.length;
    case "range":
      return present.length ? Math.max(...present) - Math.min(...present) : null;
    case "delta":
    case "diff":
      return present.length ? present[present.length - 1] - present[0] : null;
    case "lastNotNull":
    default:
      return present.length ? present[present.length - 1] : null;
  }
}

/** One value per numeric field: what a stat, a gauge, a bar gauge or a pie draws. */
export interface Reduced {
  name: string;
  field: FrameField;
  value: number | null;
  /** The field's values in time order, for a sparkline. */
  trend: { t: number; v: number | null }[];
}

/**
 * Each numeric field reduced by `reducer`; `allValues` (`reduceOptions.values: true`) gives one
 * entry per row instead, named by the frame's first string field.
 */
export function reducedOf(frames: Frame[], reducer = "lastNotNull", allValues = false): Reduced[] {
  const out: Reduced[] = [];
  for (const frame of frames) {
    const time = frame.fields.find((field) => field.type === "time");
    const label = frame.fields.find((field) => field.type === "string");
    for (const field of frame.fields) {
      if (field.type !== "number") continue;
      const values = field.values.map(numberOf);
      if (allValues) {
        values.forEach((value, index) => {
          out.push({
            name: label ? String(label.values[index] ?? "") : `${displayNameOf(field, frame)} ${index + 1}`,
            field,
            value,
            trend: [],
          });
        });
        continue;
      }
      const trend = time
        ? time.values
            .map((at, index) => ({ t: timeOf(at), v: values[index] }))
            .filter((point): point is { t: number; v: number | null } => point.t !== null)
            .sort((a, b) => a.t - b.t)
        : [];
      out.push({
        name: displayNameOf(field, frame),
        field,
        value: reduce(trend.length ? trend.map((point) => point.v) : values, reducer),
        trend,
      });
    }
  }
  return out;
}

/** Whether the frames hold anything to draw: one value in any field. */
export function hasData(frames: Frame[]): boolean {
  return frames.some((frame) => frame.fields.some((field) => field.values.some((value) => value !== null && value !== undefined)));
}
