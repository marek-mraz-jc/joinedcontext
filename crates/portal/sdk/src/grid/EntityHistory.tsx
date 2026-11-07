/**
 * How one value got to where it is (T-1431; UI-66, EP-07): the temporal answer of one attribute of
 * one entity, as a table and — for numbers — one line drawn from it.
 *
 * The read goes through the same source the grid reads, so it is the person's own session and the
 * Endpoint's Policy that decide it, temporal clamping and projection included. The series is
 * bounded by `maxPoints` before it is asked for, because one click must not ask a 16 GB node for a
 * year of a stream, and the panel says when the window was cut rather than showing a short series
 * as if it were the whole one.
 *
 * A numeric series is drawn as a line in the first colour of the design tokens' chart palette,
 * over a time axis in the page's local time and a value axis naming the unit, each point giving
 * its time and value on hover (T-2991). The table below carries every value for a screen reader.
 *
 * `ponytail: one series in SVG, no chart library in the SDK core: echarts/recharts would ship in
 * every App's grid bundle. Several series in one panel is when a library earns its place.`
 */
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { currentTokens } from "../sdk/tokens";
import { SourceError } from "./source";
import type { EntitySource, HistoryPoint, HistoryWindow } from "./source";
import { csvCell } from "../artifact";
import { historyKind, StateTimeline, valueText } from "./historyKinds";
import type { HistoryKind } from "./historyKinds";
// Its rules live beside the grid's: an application may show the history without the grid.
import "./grid.css";

/** The windows a person picks from, as hours back from now; `custom` asks for two timestamps. */
export const WINDOWS = ["hour", "day", "week", "custom"] as const;
export type Window = (typeof WINDOWS)[number];

const HOURS: Record<Exclude<Window, "custom">, number> = { hour: 1, day: 24, week: 24 * 7 };

/** The most points one read may ask for, whatever the window (EP-07). */
export const MAX_POINTS = 1000;

/** Sources that refused versions once: the next attribute starts on plain readings, no second 403. */
const versionsRefusedBy = new WeakSet<object>();

export interface HistoryLabels {
  title: string;
  window: Record<Window, string>;
  from: string;
  to: string;
  at: string;
  value: string;
  unit: string;
  empty: string;
  cut: string;
  copy: string;
  close: string;
  loading: string;
  error: string;
  /** The button that reads again after an error the server may not repeat (5xx, a timeout). */
  retry: string;
  /** What a 403 on the history read means: the role reads current values, not their history. */
  refused: string;
  /** The switch that asks for every version of each value, with who changed it and why (DM-114). */
  versions: string;
  /** What a 403 on that read means: the space's Policy grants this role no `readVersions`. */
  versionsRefused: string;
  supersededAt: string;
  changedBy: string;
  reason: string;
}

export const DEFAULT_HISTORY_LABELS: HistoryLabels = {
  title: "History",
  window: { hour: "Last hour", day: "Last day", week: "Last week", custom: "Between" },
  from: "From",
  to: "To",
  at: "Observed",
  value: "Value",
  unit: "Unit",
  empty: "Nothing was recorded in this window.",
  cut: "the oldest points are not shown",
  copy: "Copy as CSV",
  close: "Close",
  loading: "Loading…",
  error: "Error",
  retry: "Try again",
  refused: "Your role may read current values but not their history; ask a steward of this space.",
  versions: "Every version, with who changed it and why",
  versionsRefused: "Your role may not read earlier versions in this space.",
  supersededAt: "Replaced",
  changedBy: "Changed by",
  reason: "Reason",
};

/** Who wrote a version, as one line: the channel, then the actor where the broker names one. */
export function changedBy(point: HistoryPoint): string {
  const { channel, actor, operation } = point.provenance ?? {};
  return [channel, operation, actor].filter((part) => part).join(" · ");
}

/** The window a choice asks for, in UTC, because a broker compares timestamps and not clocks. */
export function windowOf(
  choice: Window,
  custom: { from: string; to: string },
  maxPoints: number,
  now: () => Date = () => new Date(),
): HistoryWindow {
  if (choice === "custom") {
    return {
      from: custom.from ? new Date(custom.from).toISOString() : undefined,
      to: custom.to ? new Date(custom.to).toISOString() : undefined,
      lastN: maxPoints,
    };
  }
  const end = now();
  const start = new Date(end.getTime() - HOURS[choice] * 3600 * 1000);
  return { from: start.toISOString(), to: end.toISOString(), lastN: maxPoints };
}

/** The series as CSV, for a spreadsheet: the two columns a person came for, quoted. */
export function asCsv(points: HistoryPoint[], attr: string): string {
  const cell = (value: unknown): string => csvCell(value === null || value === undefined ? "" : String(value));
  return [`observedAt,${cell(attr)}`, ...points.map((point) => `${cell(point.at)},${cell(point.value)}`)].join("\n");
}

/** The numeric points of a series in time order, or `null` when fewer than two can be drawn. */
export function numericSeries(points: HistoryPoint[]): { at: number; value: number }[] | null {
  const numbers = points
    .filter((point) => typeof point.value === "number" || (typeof point.value === "string" && point.value.trim() !== ""))
    .map((point) => ({ at: Date.parse(point.at), value: Number(point.value) }))
    .filter((point) => Number.isFinite(point.at) && Number.isFinite(point.value))
    .sort((a, b) => a.at - b.at);
  return numbers.length < 2 ? null : numbers;
}

/** The polyline of a numeric series in a 0…100 box, or `null` when there is nothing to draw. */
export function polyline(points: HistoryPoint[]): string | null {
  const numbers = numericSeries(points);
  if (!numbers) {
    return null;
  }
  const { x, y } = scales(numbers, { left: 0, right: 100, top: 0, bottom: 100 });
  return numbers.map((point) => `${x(point.at).toFixed(2)},${y(point.value).toFixed(2)}`).join(" ");
}

interface Box {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

/** The two axes of a series in a box; SVG's y grows downward, so the largest value is at the top. */
function scales(numbers: { at: number; value: number }[], box: Box) {
  const times = numbers.map((point) => point.at);
  const values = numbers.map((point) => point.value);
  const [minAt, maxAt] = [Math.min(...times), Math.max(...times)];
  const [low, high] = [Math.min(...values), Math.max(...values)];
  const spanAt = maxAt - minAt || 1;
  const spanValue = high - low || 1;
  return {
    x: (at: number) => box.left + ((at - minAt) / spanAt) * (box.right - box.left),
    y: (value: number) => box.bottom - ((value - low) / spanValue) * (box.bottom - box.top),
    minAt,
    maxAt,
    low,
    high,
  };
}

/** The page's language: what the times and numbers are written in unless the App says. */
function pageLocale(): string | undefined {
  return (typeof document !== "undefined" && document.documentElement.lang) || undefined;
}

const H = 220;

/** The width the chart is drawn at, in CSS pixels, so its labels stay the size the page's text
 *  is at any width rather than shrinking with a scaled drawing; 600 until it is measured. */
function useWidth(): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(600);
  useEffect(() => {
    const box = ref.current;
    if (!box || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => {
      const measured = Math.round(entry.contentRect.width);
      if (measured > 0) setWidth(Math.max(measured, 240));
    });
    observer.observe(box);
    return () => observer.disconnect();
  }, []);
  return [ref, width];
}

/** One numeric series: a line in the palette's first colour on a time and a value axis. */
function Chart({
  numbers,
  label,
  unit,
  time,
  number,
}: {
  numbers: { at: number; value: number }[];
  label: string;
  unit?: string;
  time: (at: number) => string;
  number: (value: number) => string;
}) {
  const [ref, W] = useWidth();
  const PLOT: Box = { left: 56, right: W - 16, top: 24, bottom: H - 32 };
  const colour = currentTokens().chart.palette[0];
  const { x, y, minAt, maxAt, low, high } = scales(numbers, PLOT);
  // A time label needs about 80 px: five on a laptop, three on a phone.
  const count = W < 420 ? 2 : 4;
  const ticks = [...new Set(Array.from({ length: count + 1 }, (_, i) => minAt + ((maxAt - minAt) * i) / count))];
  const levels = low === high ? [low] : [low, (low + high) / 2, high];
  const line = numbers.map((point) => `${x(point.at).toFixed(1)},${y(point.value).toFixed(1)}`).join(" ");
  return (
    <div ref={ref} className="jc-grid-history-plot">
      <svg className="jc-grid-history-chart" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={label}>
        {unit && (
          <text className="jc-grid-history-unit" x={PLOT.left} y={PLOT.top - 10}>
            {unit}
          </text>
        )}
        {levels.map((level) => (
          <g key={level}>
            <line className="jc-grid-history-grid" x1={PLOT.left} x2={PLOT.right} y1={y(level)} y2={y(level)} />
            <text className="jc-grid-history-tick" x={PLOT.left - 6} y={y(level) + 4} textAnchor="end">
              {number(level)}
            </text>
          </g>
        ))}
        <line className="jc-grid-history-axis" x1={PLOT.left} x2={PLOT.right} y1={PLOT.bottom} y2={PLOT.bottom} />
        {ticks.map((at, i) => (
          <text
            key={at}
            className="jc-grid-history-tick"
            x={x(at)}
            y={H - 10}
            textAnchor={i === 0 ? "start" : i === ticks.length - 1 ? "end" : "middle"}
          >
            {time(at)}
          </text>
        ))}
        <polyline points={line} fill="none" stroke={colour} strokeWidth="2" strokeLinejoin="round" />
        {numbers.map((point) => (
          <circle key={point.at} cx={x(point.at)} cy={y(point.value)} r="3.5" fill={colour}>
            <title>{`${time(point.at)}: ${number(point.value)}${unit ? ` ${unit}` : ""}`}</title>
          </circle>
        ))}
      </svg>
    </div>
  );
}

export interface EntityHistoryProps {
  source: Pick<EntitySource, "history">;
  id: string;
  attr: string;
  /** What the value is measured in, as the cell showed it. */
  unit?: string;
  /** The panel's heading, in the App's words; the default is the title and the attribute. The
   *  entity's id is never shown: it is what a person cannot read (T-2991). */
  heading?: string;
  /** The language times and numbers are written in; the page's own by default. */
  locale?: string;
  /** The most decimals a value is written with: the attribute's own precision. */
  fractionDigits?: number;
  maxPoints?: number;
  labels?: Partial<HistoryLabels>;
  /** Offer the switch to every version of each value; the gateway decides who gets them (DM-114). */
  versions?: boolean;
  /** Start on every version (an attribute's own view: who changed it and when, owner 2026-11-01). */
  versionsFirst?: boolean;
  /** A host's own picture of a kind of history, in place of the built-in one (null draws only the table). */
  views?: Partial<Record<HistoryKind, (points: HistoryPoint[]) => React.ReactNode>>;
  onClose?: () => void;
  now?: () => Date;
}

export function EntityHistory(props: EntityHistoryProps): React.JSX.Element {
  const { source, id, attr, unit, onClose, now } = props;
  const maxPoints = Math.min(props.maxPoints ?? MAX_POINTS, MAX_POINTS);
  const labels = useMemo(
    (): HistoryLabels => ({
      ...DEFAULT_HISTORY_LABELS,
      ...props.labels,
      window: { ...DEFAULT_HISTORY_LABELS.window, ...props.labels?.window },
    }),
    [props.labels],
  );

  const [choice, setChoice] = useState<Window>("day");
  const [custom, setCustom] = useState({ from: "", to: "" });
  const [points, setPoints] = useState<HistoryPoint[] | null>(null);
  const [error, setError] = useState<{ text: string; retry: boolean } | null>(null);
  const [allVersions, setAllVersions] = useState(Boolean(props.versionsFirst) && !versionsRefusedBy.has(props.source));
  const [versionsDenied, setVersionsDenied] = useState(versionsRefusedBy.has(props.source));

  const read = useCallback(async () => {
    if (!source.history) {
      return;
    }
    setError(null);
    setPoints(null);
    try {
      const window = windowOf(choice, custom, maxPoints, now);
      const got = await source.history(id, attr, allVersions ? { ...window, versions: true } : window);
      setPoints(got);
    } catch (err) {
      // A refusal names the missing right and offers no retry: reading again cannot change it.
      const text = err instanceof Error && err.message ? err.message : "";
      if (err instanceof SourceError && err.status === 403) {
        if (allVersions) {
          // Versions refused: say so beside the switch and show the readings the role may read.
          versionsRefusedBy.add(source);
          setVersionsDenied(true);
          setAllVersions(false);
          return;
        }
        setError({ text: text ? `${labels.refused} (${text})` : labels.refused, retry: false });
      } else {
        setError({ text: text ? `${labels.error}: ${text}` : labels.error, retry: true });
      }
    }
  }, [source, id, attr, choice, custom, maxPoints, now, allVersions, labels.error, labels.refused]);

  useEffect(() => {
    void read();
  }, [read]);

  const locale = props.locale ?? pageLocale();
  const digits = props.fractionDigits ?? 2;
  const { time, stamp, number } = useMemo(() => {
    const numbers = new Intl.NumberFormat(locale, { maximumFractionDigits: digits });
    // A window longer than a day names the day on the axis, or a week reads as one day's clock.
    const clock = new Intl.DateTimeFormat(
      locale,
      choice === "hour" || choice === "day"
        ? { hour: "2-digit", minute: "2-digit" }
        : { day: "numeric", month: "numeric", hour: "2-digit", minute: "2-digit" },
    );
    const full = new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" });
    return {
      time: (at: number) => clock.format(at),
      stamp: (at: string) => {
        const parsed = Date.parse(at);
        return Number.isFinite(parsed) ? full.format(parsed) : at;
      },
      number: (value: number) => numbers.format(value),
    };
  }, [locale, digits, choice]);
  const heading = props.heading ?? `${labels.title}: ${attr}`;
  // The line follows the values that hold today: a replaced version is a row of the table only.
  const series = points ? numericSeries(points.filter((point) => !point.supersededAt)) : null;
  const shown = (value: unknown): string => valueText(value, number);
  const current = points ? points.filter((point) => !point.supersededAt) : [];
  const kind = historyKind(current);

  return (
    <section className="jc-grid-history" aria-label={heading}>
      <header>
        <h3>{heading}</h3>
        {onClose && (
          <button type="button" onClick={onClose}>
            {labels.close}
          </button>
        )}
      </header>

      <fieldset>
        {WINDOWS.map((each) => (
          <label key={each}>
            <input
              type="radio"
              name={`jc-history-${attr}`}
              checked={choice === each}
              onChange={() => setChoice(each)}
            />
            {labels.window[each]}
          </label>
        ))}
      </fieldset>

      {props.versions && (
        <label className="jc-grid-history-versions">
          <input type="checkbox" checked={allVersions} disabled={versionsDenied} onChange={(e) => setAllVersions(e.target.checked)} />
          {labels.versions}
        </label>
      )}
      {props.versions && versionsDenied && <p className="jc-grid-history-note">{labels.versionsRefused}</p>}

      {choice === "custom" && (
        <fieldset>
          <label>
            {labels.from}
            <input
              type="datetime-local"
              value={custom.from}
              onChange={(e) => setCustom({ ...custom, from: e.target.value })}
            />
          </label>
          <label>
            {labels.to}
            <input
              type="datetime-local"
              value={custom.to}
              onChange={(e) => setCustom({ ...custom, to: e.target.value })}
            />
          </label>
        </fieldset>
      )}

      {error && (
        <p className="jc-grid-error" role="alert">
          {error.text}
          {error.retry && (
            <button type="button" onClick={() => void read()}>
              {labels.retry}
            </button>
          )}
        </p>
      )}
      {!error && points === null && <p>{labels.loading}</p>}
      {points !== null && points.length === 0 && <p>{labels.empty}</p>}

      {points !== null && points.length > 0 && (
        <>
          {points.length >= maxPoints && <p className="jc-grid-history-cut">{labels.cut}</p>}
          {kind && props.views?.[kind] ? (
            props.views[kind]!(current)
          ) : kind === "number" ? (
            series && <Chart numbers={series} label={heading} unit={unit} time={time} number={number} />
          ) : kind === "boolean" || kind === "state" ? (
            <StateTimeline points={current} label={heading} time={time} text={shown} now={(now?.() ?? new Date()).getTime()} />
          ) : null}
          <table>
            <thead>
              <tr>
                <th>{labels.at}</th>
                <th>{labels.value}</th>
                {unit && <th>{labels.unit}</th>}
                {allVersions && (
                  <>
                    <th>{labels.supersededAt}</th>
                    <th>{labels.changedBy}</th>
                    <th>{labels.reason}</th>
                  </>
                )}
              </tr>
            </thead>
            <tbody>
              {points.map((point, index) => (
                <tr key={`${point.at}-${index}`}>
                  <td>
                    <time dateTime={point.at}>{stamp(point.at)}</time>
                  </td>
                  <td>{shown(point.value)}</td>
                  {unit && <td>{unit}</td>}
                  {allVersions && (
                    <>
                      <td>{point.supersededAt ? <time dateTime={point.supersededAt}>{stamp(point.supersededAt)}</time> : ""}</td>
                      <td>{changedBy(point)}</td>
                      <td>{point.provenance?.reason ?? ""}</td>
                    </>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
          <button type="button" onClick={() => void navigator.clipboard?.writeText(asCsv(points, attr))}>
            {labels.copy}
          </button>
        </>
      )}
    </section>
  );
}
