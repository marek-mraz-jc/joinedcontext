import type { Row } from "../ngsi";
import { aggregate, format } from "../ngsi";
import type { StatItem } from "../spec";
import { unitSymbol } from "../sdk/units";

/** A row of tiles: one number each, over the rows the filters left. */
export function Stats({ rows, items, units = {} }: { rows: Row[]; items: StatItem[]; units?: Record<string, string> }) {
  return (
    <div className="stats">
      {items.map((item) => {
        const value = aggregate(rows, item.agg, item.attr);
        // The spec's own unit wins; else the model's, for any aggregate but a count (DM-06).
        const unit = item.unit ?? (item.agg !== "count" && item.attr ? unitSymbol(units[item.attr]) : "");
        return (
          <div className="stat" key={`${item.label}-${item.agg}-${item.attr ?? ""}`}>
            <span className="stat-value">
              {value === null ? "–" : format(value)}
              {unit && value !== null ? <span className="stat-unit"> {unit}</span> : null}
            </span>
            <span className="stat-label">{item.label}</span>
          </div>
        );
      })}
    </div>
  );
}
