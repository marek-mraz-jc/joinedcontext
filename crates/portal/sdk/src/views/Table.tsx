import { useMemo, useState } from "react";
import type { Column, Row } from "../ngsi";
import { columnKind, format } from "../ngsi";
import { unitSymbol } from "../sdk/units";

const PAGE_SIZE = 50;

export function compare(a: Row, b: Row, attr: string, dir: "asc" | "desc"): number {
  const x = a[attr];
  const y = b[attr];
  let order = 0;
  if (x === null || x === undefined) {
    order = y === null || y === undefined ? 0 : 1;
  } else if (y === null || y === undefined) {
    order = -1;
  } else if (typeof x === "number" && typeof y === "number") {
    order = x - y;
  } else {
    order = format(x).localeCompare(format(y), undefined, { numeric: true });
  }
  return dir === "asc" ? order : -order;
}

/** A sortable, paged table; a click on a row selects it for the detail view. */
export function Table({
  rows,
  columns,
  sort,
  selected,
  onSelect,
  units = {},
}: {
  rows: Row[];
  columns: string[];
  /** The unit of each quantity column, by attribute, which its header names (DM-06). */
  units?: Record<string, string>;
  sort?: { attr: string; dir: "asc" | "desc" };
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  const [order, setOrder] = useState<{ attr: string; dir: "asc" | "desc" } | null>(sort ?? null);
  const [page, setPage] = useState(0);
  const kinds = useMemo(() => Object.fromEntries(columns.map((c) => [c, columnKind(rows, c)])) as Record<string, Column>, [rows, columns]);
  const sorted = useMemo(() => (order ? [...rows].sort((a, b) => compare(a, b, order.attr, order.dir)) : rows), [rows, order]);
  const pages = Math.max(1, Math.ceil(sorted.length / PAGE_SIZE));
  const current = Math.min(page, pages - 1);
  const slice = sorted.slice(current * PAGE_SIZE, (current + 1) * PAGE_SIZE);

  const toggle = (attr: string) => {
    setOrder((o) => (o?.attr === attr && o.dir === "asc" ? { attr, dir: "desc" } : { attr, dir: "asc" }));
    setPage(0);
  };

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            {columns.map((column) => (
              <th key={column} scope="col" aria-sort={order?.attr === column ? (order.dir === "asc" ? "ascending" : "descending") : "none"}>
                <button type="button" onClick={() => toggle(column)}>
                  {column}
                  {unitSymbol(units[column]) ? ` (${unitSymbol(units[column])})` : ""}
                  {order?.attr === column ? (order.dir === "asc" ? " ▲" : " ▼") : ""}
                </button>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {slice.map((row) => (
            <tr
              key={row.id}
              className={row.id === selected ? "selected" : undefined}
              onClick={() => onSelect(row.id)}
              tabIndex={0}
              onKeyDown={(event) => {
                if (event.key === "Enter") onSelect(row.id);
              }}
            >
              {columns.map((column) => (
                <td key={column} className={kinds[column] === "number" ? "num" : undefined}>
                  {format(row[column], kinds[column])}
                </td>
              ))}
            </tr>
          ))}
          {slice.length === 0 && (
            <tr>
              <td colSpan={columns.length} className="empty">No rows match the filters.</td>
            </tr>
          )}
        </tbody>
      </table>
      {pages > 1 && (
        <div className="pager">
          <button type="button" disabled={current === 0} onClick={() => setPage(current - 1)}>‹</button>
          <span>{current + 1} / {pages}</span>
          <button type="button" disabled={current >= pages - 1} onClick={() => setPage(current + 1)}>›</button>
        </div>
      )}
    </div>
  );
}
