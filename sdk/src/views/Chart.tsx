import { useMemo } from "react";
import type { Row } from "../ngsi";
import { aggregate, format } from "../ngsi";
import type { Agg } from "../spec";

export interface Point {
  x: string;
  y: number;
}

/** The series a chart draws: `y` per distinct `x`, aggregated, the largest `top` kept. */
export function series(rows: Row[], x: string, y: string, agg: Agg = "sum", top = 20, type: "bar" | "line" | "pie" = "bar"): Point[] {
  const groups = new Map<string, Row[]>();
  for (const row of rows) {
    const key = format(row[x]);
    if (key === "") continue;
    groups.set(key, [...(groups.get(key) ?? []), row]);
  }
  let points = [...groups.entries()]
    .map(([key, group]) => ({ x: key, y: aggregate(group, agg, y) }))
    .filter((p): p is Point => p.y !== null);
  if (type === "line") {
    points.sort((a, b) => a.x.localeCompare(b.x, undefined, { numeric: true }));
    return points.slice(-Math.max(top, 2) * 5);
  }
  points.sort((a, b) => b.y - a.y);
  points = points.slice(0, top);
  return points;
}

const W = 640;
const H = 280;
const PAD = { top: 12, right: 12, bottom: 64, left: 56 };
const PIE = ["#0f766e", "#2563eb", "#d97706", "#dc2626", "#7c3aed", "#059669", "#db2777", "#4b5563"];

/** Inline SVG, so a chart needs no library and no network. */
export function Chart({ rows, x, y, agg, top, type, accent }: { rows: Row[]; x: string; y: string; agg?: Agg; top?: number; type: "bar" | "line" | "pie"; accent: string }) {
  const points = useMemo(() => series(rows, x, y, agg, top, type), [rows, x, y, agg, top, type]);
  if (points.length === 0) {
    return <p className="empty">Nothing to chart for the current filters.</p>;
  }
  const label = `${agg ?? "sum"} of ${y} by ${x}`;
  if (type === "pie") {
    const total = points.reduce((a, p) => a + p.y, 0) || 1;
    let angle = -Math.PI / 2;
    const arcs = points.map((p, i) => {
      const start = angle;
      angle += (p.y / total) * Math.PI * 2;
      const large = angle - start > Math.PI ? 1 : 0;
      const [x0, y0] = [Math.cos(start), Math.sin(start)];
      const [x1, y1] = [Math.cos(angle), Math.sin(angle)];
      return { ...p, d: `M0 0 L${x0} ${y0} A1 1 0 ${large} 1 ${x1} ${y1} Z`, color: PIE[i % PIE.length] };
    });
    return (
      <div className="chart">
        <svg viewBox="-1.2 -1.2 2.4 2.4" role="img" aria-label={label} className="pie">
          {arcs.map((arc) => (
            <path key={arc.x} d={arc.d} fill={arc.color}>
              <title>{`${arc.x}: ${format(arc.y)}`}</title>
            </path>
          ))}
        </svg>
        <ul className="legend">
          {arcs.map((arc) => (
            <li key={arc.x}><span className="swatch" style={{ background: arc.color }} />{arc.x} · {format(arc.y)}</li>
          ))}
        </ul>
      </div>
    );
  }
  const max = Math.max(...points.map((p) => p.y), 0);
  const min = Math.min(...points.map((p) => p.y), 0);
  const span = max - min || 1;
  const innerW = W - PAD.left - PAD.right;
  const innerH = H - PAD.top - PAD.bottom;
  const yOf = (v: number) => PAD.top + innerH - ((v - min) / span) * innerH;
  const step = innerW / points.length;
  const ticks = [min, min + span / 2, max];
  return (
    <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label={label} className="chart">
      {ticks.map((tick) => (
        <g key={tick}>
          <line x1={PAD.left} x2={W - PAD.right} y1={yOf(tick)} y2={yOf(tick)} className="grid" />
          <text x={PAD.left - 6} y={yOf(tick) + 4} textAnchor="end" className="tick">{format(tick)}</text>
        </g>
      ))}
      {type === "bar"
        ? points.map((p, i) => (
            <rect key={p.x} x={PAD.left + i * step + step * 0.15} width={step * 0.7} y={Math.min(yOf(p.y), yOf(0))} height={Math.abs(yOf(0) - yOf(p.y))} fill={accent}>
              <title>{`${p.x}: ${format(p.y)}`}</title>
            </rect>
          ))
        : (
            <polyline fill="none" stroke={accent} strokeWidth={2} points={points.map((p, i) => `${PAD.left + i * step + step / 2},${yOf(p.y)}`).join(" ")} />
          )}
      {points.map((p, i) =>
        points.length <= 30 || i % Math.ceil(points.length / 30) === 0 ? (
          <text key={p.x} x={PAD.left + i * step + step / 2} y={H - PAD.bottom + 14} className="tick" transform={`rotate(-40 ${PAD.left + i * step + step / 2} ${H - PAD.bottom + 14})`} textAnchor="end">
            {p.x.length > 14 ? `${p.x.slice(0, 13)}…` : p.x}
          </text>
        ) : null,
      )}
    </svg>
  );
}
