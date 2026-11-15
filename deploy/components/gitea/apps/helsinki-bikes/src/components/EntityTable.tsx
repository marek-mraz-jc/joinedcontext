import { useMemo, useRef, useState, type ReactNode } from "react";
import { columnKind, compare, format } from "@joinedcontext/sdk";
import type { ProblemError, Row } from "@joinedcontext/sdk";
import { Empty, Loading, Problem } from "@joinedcontext/sdk";

export interface ColumnDef<T extends Row = Row> {
  attr: string;
  label?: string;
  render?: (row: T) => ReactNode;
}

export function defaultColumns(rows: Row[], max = 8): string[] {
  if (max <= 0) return [];
  const sample = rows.slice(0, 50);
  const seen = new Set<string>();
  const cols: string[] = [];

  for (const row of sample) {
    for (const key of Object.keys(row)) {
      if (key === "id" || key === "type" || key === "@context") continue;
      if (seen.has(key)) continue;
      seen.add(key);
      if (columnKind(sample, key) !== "geo") {
        cols.push(key);
        if (cols.length >= max) return cols;
      }
    }
  }

  return cols;
}

export function EntityTable<T extends Row = Row>({
  rows,
  columns,
  pageSize = 50,
  selected = null,
  onSelect,
  initialSort,
  loading = false,
  error = null,
  empty,
  caption,
}: {
  rows: T[];
  columns?: Array<string | ColumnDef<T>>;
  pageSize?: number;
  selected?: string | null;
  onSelect?: (row: T) => void;
  initialSort?: { attr: string; dir: "asc" | "desc" };
  loading?: boolean;
  error?: ProblemError | Error | null;
  empty?: ReactNode;
  caption?: string;
}): React.JSX.Element {
  const resolvedColumns = useMemo<ColumnDef<T>[]>(() => {
    const raw = columns ?? defaultColumns(rows);
    return raw.map((c) => (typeof c === "string" ? { attr: c, label: c } : c));
  }, [columns, rows]);

  const colKinds = useMemo(() => {
    const map: Record<string, string> = {};
    for (const col of resolvedColumns) {
      map[col.attr] = columnKind(rows, col.attr);
    }
    return map;
  }, [resolvedColumns, rows]);

  const [sort, setSort] = useState<{ attr: string; dir: "asc" | "desc" } | null>(initialSort ?? null);
  const [page, setPage] = useState(1);

  const prevLenRef = useRef(rows.length);
  if (prevLenRef.current !== rows.length) {
    prevLenRef.current = rows.length;
    if (page !== 1) {
      setPage(1);
    }
  }

  const toggleSort = (attr: string) => {
    setSort((prev) => {
      if (prev?.attr === attr) {
        return { attr, dir: prev.dir === "asc" ? "desc" : "asc" };
      }
      return { attr, dir: "asc" };
    });
    setPage(1);
  };

  const sorted = useMemo(() => {
    if (!sort) return rows;
    return [...rows].sort((a, b) => compare(a, b, sort.attr, sort.dir));
  }, [rows, sort]);

  const totalPages = Math.max(1, Math.ceil(sorted.length / pageSize));
  const safePage = Math.min(page, totalPages);

  const pagedRows = useMemo(() => {
    const start = (safePage - 1) * pageSize;
    return sorted.slice(start, start + pageSize);
  }, [sorted, safePage, pageSize]);

  // After every hook (T-3418): React keeps no hook state across a render that called none, so an
  // early return above them forgot the sort and the page whenever a filter emptied the table.
  if (error) {
    return <Problem error={error} />;
  }

  if (loading && rows.length === 0) {
    return <Loading />;
  }

  if (rows.length === 0) {
    return <Empty>{empty}</Empty>;
  }

  return (
    <div className="jc-table-wrap">
      {/* The roles are spelled out: the narrow layout sets display on the table's parts, and
          some screen readers drop table semantics with it. */}
      <table className="jc-table" role="table">
        {caption && <caption>{caption}</caption>}
        <thead>
          <tr role="row">
            {resolvedColumns.map((col) => {
              const isNum = colKinds[col.attr] === "number";
              const sortDir = sort?.attr === col.attr ? (sort.dir === "asc" ? "ascending" : "descending") : "none";
              return (
                <th
                  key={col.attr}
                  role="columnheader"
                  scope="col"
                  aria-sort={sortDir}
                  className={isNum ? "jc-num" : undefined}
                >
                  <button type="button" onClick={() => toggleSort(col.attr)}>
                    {col.label ?? col.attr}
                    {/* aria-sort says the direction; the arrow is for the eye. */}
                    {sort?.attr === col.attr && <span aria-hidden="true">{sort.dir === "asc" ? " ▲" : " ▼"}</span>}
                  </button>
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {pagedRows.map((row) => (
            <tr
              key={row.id}
              role="row"
              aria-selected={row.id === selected}
              tabIndex={onSelect ? 0 : undefined}
              onClick={() => onSelect?.(row)}
              onKeyDown={(e) => {
                if (e.key === "Enter") onSelect?.(row);
              }}
            >
              {resolvedColumns.map((col) => {
                const kind = colKinds[col.attr] as "number" | "date" | "geo" | "text";
                return (
                  <td
                    key={col.attr}
                    role="cell"
                    className={kind === "number" ? "jc-num" : undefined}
                    data-label={col.label ?? col.attr}
                  >
                    {col.render ? col.render(row) : format(row[col.attr], kind)}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
      {totalPages > 1 && (
        <div className="jc-pager">
          <button
            type="button"
            disabled={safePage === 1}
            onClick={() => setPage((p) => Math.max(1, p - 1))}
          >
            Previous
          </button>
          <span>
            Page {safePage} of {totalPages}
          </span>
          <button
            type="button"
            disabled={safePage >= totalPages}
            onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
          >
            Next
          </button>
        </div>
      )}
    </div>
  );
}
