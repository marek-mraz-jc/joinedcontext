import type { Row } from "../ngsi";
import { format } from "../ngsi";
import { unitSymbol } from "../sdk/units";

/** The selected entity, every attribute the source asked for as a definition list. */
export function Detail({ row, attrs, units = {} }: { row: Row | null; attrs: string[]; units?: Record<string, string> }) {
  if (!row) {
    return <p className="empty">Pick a row in a table or a point on the map.</p>;
  }
  return (
    <dl className="detail">
      <dt>id</dt>
      <dd className="mono">{row.id}</dd>
      {attrs.map((attr) => (
        <div key={attr} className="detail-row">
          <dt>{attr}</dt>
          <dd>{format(row[attr]) ? `${format(row[attr])}${unitSymbol(units[attr]) ? ` ${unitSymbol(units[attr])}` : ""}` : "–"}</dd>
        </div>
      ))}
    </dl>
  );
}
