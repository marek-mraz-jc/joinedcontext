/**
 * One chart of the records (T-2966, AP-123): a line over the periods, a column per key, or the
 * territories as bars, largest first.
 *
 * Drawn as plain SVG and HTML, no chart library: the lane bakes none this App names, and a line
 * of a few hundred points needs a polyline, not a dependency. The colour is the one the card was
 * handed from the design tokens' chart palette, never a value the data carries. The numbers are
 * also a table under the chart, which is what a screen reader reads.
 */
import type { CSSProperties } from "react";
import type { Chart, Shape } from "./series";

const W = 320;
const H = 140;
const PAD = { top: 8, right: 8, bottom: 22, left: 8 };

export interface ChartWords {
  locale: string;
  latest: string;
  values: string;
  /** The heading of the first column of the values: a period, a territory or a key. */
  axis: Record<Shape, string>;
  value: string;
}

export function SeriesChart({ chart, colour, words }: { chart: Chart; colour: string; words: ChartWords }) {
  const format = new Intl.NumberFormat(words.locale, { maximumFractionDigits: 2 });
  const last = chart.points.at(-1);
  const style = { "--series": colour } as CSSProperties;
  const captionId = `chart-${chart.id}`;
  return (
    <figure className="series-card" style={style} aria-labelledby={captionId}>
      <figcaption id={captionId}>
        <span className="series-title">{chart.title}</span>
        <span className="series-subtitle">{chart.subtitle}</span>
      </figcaption>
      {chart.shape === "line" && last ? (
        <p className="series-latest">
          <span className="series-number">{format.format(last.value)}</span> {chart.unit}
          <span className="series-when">
            {" "}
            · {words.latest} {last.label}
          </span>
        </p>
      ) : null}
      {chart.shape === "areas" ? <Bars chart={chart} format={format} /> : <Plot chart={chart} />}
      <details className="series-values">
        <summary>{words.values}</summary>
        <table>
          <thead>
            <tr>
              <th scope="col">{words.axis[chart.shape]}</th>
              <th scope="col">
                {words.value} ({chart.unit})
              </th>
            </tr>
          </thead>
          <tbody>
            {chart.points.map((point) => (
              <tr key={point.at}>
                <th scope="row">{point.label}</th>
                <td>{format.format(point.value)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </details>
    </figure>
  );
}

/**
 * A line (over the periods) or columns (one per key). Columns stand on zero, since their height is
 * the number; a line spans its own range, so a change of two per cent is not drawn flat.
 */
function Plot({ chart }: { chart: Chart }) {
  const values = chart.points.map((point) => point.value);
  const [min, max] = [Math.min(...values), Math.max(...values)];
  const pad = chart.shape === "line" ? Math.max((max - min) * 0.1, Math.abs(max) * 0.01, 1) : 0;
  const low = chart.shape === "line" ? min - pad : Math.min(0, min);
  const high = chart.shape === "line" ? max + pad : Math.max(max, low + 1);
  const n = chart.points.length;
  const x = (i: number) => PAD.left + (n <= 1 ? (W - PAD.left - PAD.right) / 2 : (i * (W - PAD.left - PAD.right)) / (n - 1));
  const y = (value: number) => PAD.top + ((high - value) * (H - PAD.top - PAD.bottom)) / (high - low);
  // At most five labels along the axis, the first and the last always among them.
  const ticks = [...new Set([0, Math.round((n - 1) / 4), Math.round((n - 1) / 2), Math.round((3 * (n - 1)) / 4), n - 1])].filter(
    (i) => i >= 0 && i < n,
  );
  const width = Math.max(1, (W - PAD.left - PAD.right) / Math.max(n, 1) - 1);
  return (
    <svg className="series-plot" viewBox={`0 0 ${W} ${H}`} aria-hidden="true" focusable="false">
      <line className="axis" x1={PAD.left} x2={W - PAD.right} y1={H - PAD.bottom} y2={H - PAD.bottom} />
      {chart.shape === "keys" ? (
        chart.points.map((point, i) => (
          <rect
            key={point.at}
            className="column"
            x={PAD.left + i * (width + 1)}
            y={y(Math.max(point.value, 0))}
            width={width}
            height={Math.abs(y(point.value) - y(0))}
          />
        ))
      ) : (
        <>
          <polyline className="line" points={chart.points.map((point, i) => `${x(i)},${y(point.value)}`).join(" ")} />
          {n > 0 ? <circle className="dot" cx={x(n - 1)} cy={y(chart.points[n - 1].value)} r={3.5} /> : null}
        </>
      )}
      {ticks.map((i) => (
        <text
          key={i}
          className="tick"
          x={chart.shape === "keys" ? PAD.left + i * (width + 1) + width / 2 : x(i)}
          y={H - 6}
          textAnchor={i === 0 ? "start" : i === n - 1 ? "end" : "middle"}
        >
          {chart.points[i].label}
        </text>
      ))}
    </svg>
  );
}

/** The territories as bars, largest first: a list, so each name and number stay at reading size. */
function Bars({ chart, format }: { chart: Chart; format: Intl.NumberFormat }) {
  const max = Math.max(0, ...chart.points.map((point) => point.value));
  return (
    <ol className="series-bars" aria-hidden="true">
      {chart.points.map((point) => (
        <li key={point.at}>
          <span className="bar-label">{point.label}</span>
          <span className="bar-track">
            <span className="bar" style={{ "--share": max > 0 ? Math.max(0, point.value / max) : 0 } as CSSProperties} />
          </span>
          <span className="bar-value">
            {format.format(point.value)} {chart.unit}
          </span>
        </li>
      ))}
    </ol>
  );
}
