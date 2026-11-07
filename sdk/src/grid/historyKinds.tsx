/**
 * How an attribute's history is drawn depends on what its values are (owner 2026-11-01): numbers
 * as a line, booleans and short texts or enum values as a band of states over time, places and
 * structured values in the table only. `historyKind` decides from the values themselves; a host
 * replaces any kind's picture through `EntityHistory`'s `views`.
 */
import React from "react";
import { currentTokens } from "../sdk/tokens";
import type { HistoryPoint } from "./source";

export type HistoryKind = "number" | "boolean" | "state" | "relationship" | "geo" | "language" | "structured" | "text";

/** At most this many distinct values still read as states; more is free text, shown as a table. */
export const MAX_STATES = 12;

const isGeo = (v: unknown): boolean =>
  typeof v === "object" && v !== null && typeof (v as { type?: unknown }).type === "string" && "coordinates" in (v as object);

/** The kind of a history from its values: the one kind every value shares, else "text" or "structured". */
export function historyKind(points: HistoryPoint[]): HistoryKind | null {
  const values = points.map((point) => point.value).filter((value) => value !== null && value !== undefined);
  if (values.length === 0) return null;
  if (values.every((v) => typeof v === "number" || (typeof v === "string" && v.trim() !== "" && Number.isFinite(Number(v))))) return "number";
  if (values.every((v) => typeof v === "boolean")) return "boolean";
  if (values.every(isGeo)) return "geo";
  if (values.every((v) => typeof v === "string" && /^urn:/i.test(v))) return "relationship";
  if (values.every((v) => typeof v === "object" && v !== null && !Array.isArray(v) && Object.values(v).every((t) => typeof t === "string"))) return "language";
  if (values.some((v) => typeof v === "object")) return "structured";
  // A state comes back: a value seen again (or an enum's few values). Every value new is free text.
  const distinct = new Set(values.map(String)).size;
  return distinct <= MAX_STATES && distinct < values.length ? "state" : "text";
}

/** A value as one line of text for the table, whatever its kind. */
export function valueText(value: unknown, number: (n: number) => string, yes = "yes", no = "no"): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "number") return number(value);
  if (typeof value === "boolean") return value ? yes : no;
  if (isGeo(value)) {
    const geo = value as { type: string; coordinates: unknown };
    return geo.type === "Point" && Array.isArray(geo.coordinates) ? `${geo.type} ${(geo.coordinates as number[]).map(number).join(", ")}` : geo.type;
  }
  if (typeof value === "object") return JSON.stringify(value);
  return String(value);
}

export interface Segment {
  value: string;
  from: number;
  to: number;
}

/** Consecutive equal values as segments over time, oldest first; the last runs to `end`. */
export function segmentsOf(points: HistoryPoint[], end: number): Segment[] {
  const sorted = points
    .map((point) => ({ at: Date.parse(point.at), value: String(point.value) }))
    .filter((point) => Number.isFinite(point.at))
    .sort((a, b) => a.at - b.at);
  const out: Segment[] = [];
  sorted.forEach((point, i) => {
    const to = sorted[i + 1]?.at ?? Math.max(end, point.at);
    const last = out[out.length - 1];
    if (last && last.value === point.value) last.to = to;
    else out.push({ value: point.value, from: point.at, to });
  });
  return out;
}

/** Booleans and states as a band of coloured segments, with a legend naming each value and its share of time. */
export function StateTimeline({
  points,
  label,
  time,
  text,
  now,
}: {
  points: HistoryPoint[];
  label: string;
  time: (at: number) => string;
  text: (value: unknown) => string;
  now: number;
}): React.JSX.Element | null {
  const segments = segmentsOf(points, now);
  if (segments.length === 0) return null;
  const start = segments[0].from;
  const span = Math.max(1, segments[segments.length - 1].to - start);
  const palette = currentTokens().chart.palette;
  const values = [...new Set(segments.map((s) => s.value))];
  const colour = (value: string) => palette[values.indexOf(value) % palette.length];
  const raw = new Map(points.map((p) => [String(p.value), p.value]));
  const share = (value: string) => segments.filter((s) => s.value === value).reduce((sum, s) => sum + s.to - s.from, 0) / span;
  return (
    <figure className="jc-grid-history-states" aria-label={label}>
      <div className="jc-grid-history-band" role="img" aria-label={label}>
        {segments.map((s) => (
          <span
            key={`${s.from}-${s.value}`}
            style={{ left: `${((s.from - start) / span) * 100}%`, width: `${((s.to - s.from) / span) * 100}%`, background: colour(s.value) }}
            title={`${text(raw.get(s.value))}: ${time(s.from)} – ${time(s.to)}`}
          />
        ))}
      </div>
      <div className="jc-grid-history-band-axis">
        <span>{time(start)}</span>
        <span>{time(start + span)}</span>
      </div>
      <figcaption>
        {values.map((value) => (
          <span key={value} className="jc-grid-history-legend">
            <i style={{ background: colour(value) }} />
            {text(raw.get(value))} <small>{Math.round(share(value) * 100)} %</small>
          </span>
        ))}
      </figcaption>
    </figure>
  );
}
