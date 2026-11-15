/**
 * One day of PM10 and PM2.5 at one station, with each pollutant's EU limit as a dashed line
 * (T-2925). ponytail: an SVG line chart, two series and two reference lines; a chart library when
 * a screen wants zoom, brushing or more than a handful of series.
 */
import type { JSX } from "react";
import { rechartsPalette } from "@joinedcontext/sdk";
import { LIMITS } from "./quality";
import type { History, Pollutant } from "./quality";

const WIDTH = 640;
const HEIGHT = 240;
const PAD = { top: 16, right: 16, bottom: 28, left: 40 };

const NAME: Record<Pollutant, string> = { pm10: "PM10", pm25: "PM2.5" };
const LIMIT_NAME: Record<Pollutant, string> = { pm10: "PM10 daily limit", pm25: "PM2.5 annual limit" };

function hour(time: number): string {
  return new Date(time).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

export function Chart({ history, station }: { history: History; station: string }): JSX.Element {
  const pollutants = (Object.keys(LIMITS) as Pollutant[]).filter((p) => history[p].length > 0);
  if (pollutants.length === 0) {
    return <p role="status">No PM10 or PM2.5 readings from {station} in the last 24 hours.</p>;
  }
  // The token palette colours the series (AP-123): the first colour is PM10, the second PM2.5.
  const [first, second] = rechartsPalette();
  const colour: Record<Pollutant, string> = { pm10: first, pm25: second };

  const points = pollutants.flatMap((p) => history[p]);
  const start = Math.min(...points.map((p) => p.time));
  const end = Math.max(...points.map((p) => p.time));
  const top = Math.max(...points.map((p) => p.value), ...pollutants.map((p) => LIMITS[p])) * 1.1;
  const x = (time: number) =>
    PAD.left + (end === start ? 0.5 : (time - start) / (end - start)) * (WIDTH - PAD.left - PAD.right);
  const y = (value: number) => HEIGHT - PAD.bottom - (value / top) * (HEIGHT - PAD.top - PAD.bottom);
  const ticks = [0, 0.25, 0.5, 0.75, 1].map((share) => Math.round(top * share));

  const latest = pollutants
    .map((p) => `${NAME[p]} ${history[p][history[p].length - 1].value} µg/m³ (limit ${LIMITS[p]})`)
    .join(", ");

  return (
    <figure className="chart">
      <svg
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        role="img"
        aria-label={`PM readings at ${station} over the last 24 hours. Latest: ${latest}.`}
      >
        {ticks.map((tick) => (
          <g key={tick}>
            <line className="grid" x1={PAD.left} x2={WIDTH - PAD.right} y1={y(tick)} y2={y(tick)} />
            <text className="axis" x={PAD.left - 6} y={y(tick) + 4} textAnchor="end">
              {tick}
            </text>
          </g>
        ))}
        <text className="axis" x={PAD.left} y={HEIGHT - 8}>
          {hour(start)}
        </text>
        <text className="axis" x={WIDTH - PAD.right} y={HEIGHT - 8} textAnchor="end">
          {hour(end)}
        </text>
        {pollutants.map((p) => (
          <g key={p} data-series={p}>
            <line
              className="limit"
              stroke={colour[p]}
              x1={PAD.left}
              x2={WIDTH - PAD.right}
              y1={y(LIMITS[p])}
              y2={y(LIMITS[p])}
            />
            <polyline
              fill="none"
              stroke={colour[p]}
              strokeWidth={2.5}
              points={history[p].map((point) => `${x(point.time)},${y(point.value)}`).join(" ")}
            />
          </g>
        ))}
      </svg>
      <figcaption>
        <ul className="legend">
          {pollutants.map((p) => (
            <li key={p}>
              <span className="swatch" style={{ background: colour[p] }} aria-hidden="true" />
              {NAME[p]} (µg/m³), dashed: {LIMIT_NAME[p]} {LIMITS[p]}
            </li>
          ))}
        </ul>
      </figcaption>
    </figure>
  );
}
