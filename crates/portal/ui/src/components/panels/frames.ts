/**
 * daiya's data frames as the dashboard page reads them (crates/daiya/model/src/frames.rs): one
 * frame per query result, columns of `time`, `number`, `string`, `bool`, `json` or `geo` values.
 */

import { calcValueOf, reduceField, reducerId } from "./reducers";

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

/** A frame's name in a picker, Grafana's way: its name, else its query's refId, else `Series (n)`. */
export function frameName(frame: Frame, index: number): string {
  return frame.name || frame.refId || `Series (${index})`;
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

/**
 * One Grafana reducer (`options.reduceOptions.calcs`, a legend's `calcs`) over a number field's
 * values, as a number to draw: Grafana's own result (reducers.ts), a boolean as 1 or 0, anything
 * else (a text, an array, NaN or a missing result) as `null`, which the panels draw as no value.
 */
export function reduce(values: readonly (number | null)[], reducer: string): number | null {
  const value = reduceField({ values, numeric: true }, [reducer])[reducerId(reducer)];
  if (typeof value === "boolean") return value ? 1 : 0;
  return typeof value === "number" && !Number.isNaN(value) ? value : null;
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
      // The reducer meets the field as Grafana's does: NaN and infinities kept, not gaps.
      const raw = field.values.map(calcValueOf);
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
            .map((at, index) => ({ t: timeOf(at), v: values[index], raw: raw[index] }))
            .filter((point): point is { t: number; v: number | null; raw: number | null } => point.t !== null)
            .sort((a, b) => a.t - b.t)
        : [];
      out.push({
        name: displayNameOf(field, frame),
        field,
        value: reduce(trend.length ? trend.map((point) => point.raw) : raw, reducer),
        trend: trend.map(({ t, v }) => ({ t, v })),
      });
    }
  }
  return out;
}

/** Whether the frames hold anything to draw: one value in any field. */
/**
 * The span of a panel's readings when every one lies outside [from, to], as Grafana's time series
 * says "Data outside time range"; undefined when one falls inside or there is none.
 */
export function outsideRange(frames: Frame[], from: number, to: number): { from: number; to: number } | undefined {
  let first = Infinity;
  let last = -Infinity;
  for (const series of seriesOf(frames)) {
    for (const point of series.points) {
      if (point.v === null) continue;
      if (point.t >= from && point.t <= to) return undefined;
      first = Math.min(first, point.t);
      last = Math.max(last, point.t);
    }
  }
  return first <= last ? { from: first, to: last } : undefined;
}

export function hasData(frames: Frame[]): boolean {
  return frames.some((frame) => frame.fields.some((field) => field.values.some((value) => value !== null && value !== undefined)));
}
