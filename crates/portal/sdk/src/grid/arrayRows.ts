/**
 * The rows of an array grid (`DataGrid`, T-6739): a list the page already read, such as Policies or
 * Approvals, searched, filtered, sorted and paged in the browser. Only what the server returned is
 * here, so nothing in this file can show a row the person may not read; it can only narrow.
 */
import type { ReactNode } from "react";
import type { ColumnFilter } from "./filters";
import { complete } from "./filters";
import type { ColumnValues } from "./columnValues";
import { MAX_VALUES } from "./columnValues";

/** What a column holds: the words its sort uses and the filter conditions it offers. */
export type DataKind = "text" | "number" | "date" | "boolean";

export interface DataColumn<T> {
  key: string;
  label: string;
  /** The column's value as text: what search, sort, the filter and its value list read. */
  text: (row: T) => string;
  /** What the cell draws; the text when absent. */
  cell?: (row: T) => ReactNode;
  /** What the header draws instead of the label, such as a glossary term: its sort is then in the column menu. */
  heading?: ReactNode;
  kind?: DataKind;
  /** Off for a column whose text says nothing to filter by (an actions column). */
  filter?: boolean;
  sortable?: boolean;
  /** Left out until the person shows it in Arrange columns. */
  hidden?: boolean;
  align?: "left" | "right";
  /** The row's actions: no sort, no filter, no header menu and no label on a phone's card. */
  actions?: boolean;
}

export interface DataSort {
  key: string;
  dir: "asc" | "desc";
}

export interface DataQuery {
  search: string;
  filters: Record<string, ColumnFilter>;
  sort: DataSort | null;
  offset: number;
  pageSize: number;
}

export const EMPTY_QUERY: DataQuery = { search: "", filters: {}, sort: null, offset: 0, pageSize: 25 };

function asNumber(text: string): number | null {
  if (text.trim() === "") return null;
  const n = Number(text);
  return Number.isFinite(n) ? n : null;
}

function asTime(text: string): number | null {
  if (text.trim() === "") return null;
  const t = Date.parse(text);
  return Number.isNaN(t) ? null : t;
}

/** A value of `kind` as something `<` orders, or null where it is empty or unreadable. */
function ordinal(text: string, kind: DataKind): number | string | null {
  if (kind === "number") return asNumber(text);
  if (kind === "date") return asTime(text);
  return text === "" ? null : text.toLocaleLowerCase();
}

/** Whether one cell's text passes a column filter (the same conditions the entity grid asks a broker). */
export function passes(text: string, kind: DataKind, filter: ColumnFilter): boolean {
  if (!complete(filter)) return true;
  const value = text.trim();
  switch (filter.op) {
    case "empty":
      return value === "";
    case "present":
      return value !== "";
    case "anyOf":
      return (filter.values ?? []).includes(text);
    case "contains":
      return value.toLocaleLowerCase().includes(filter.value.trim().toLocaleLowerCase());
    case "equals":
    case "notEquals": {
      const left = ordinal(value, kind);
      const right = ordinal(filter.value.trim(), kind);
      const same = kind === "text" || kind === "boolean" ? value.toLocaleLowerCase() === filter.value.trim().toLocaleLowerCase() : left !== null && left === right;
      return filter.op === "equals" ? same : !same;
    }
    case "gt":
    case "gte":
    case "lt":
    case "lte":
    case "between": {
      const at = ordinal(value, kind);
      const low = ordinal(filter.value.trim(), kind);
      if (at === null || low === null) return false;
      if (filter.op === "gt") return at > low;
      if (filter.op === "gte") return at >= low;
      if (filter.op === "lt") return at < low;
      if (filter.op === "lte") return at <= low;
      const high = ordinal((filter.value2 ?? "").trim(), kind);
      return high !== null && at >= low && at <= high;
    }
    default:
      // A condition this grid does not know lets nothing through rather than everything.
      return false;
  }
}

/** The rows a query keeps, before paging: search over every shown column, then each filter. */
export function filterRows<T>(rows: readonly T[], columns: readonly DataColumn<T>[], query: Pick<DataQuery, "search" | "filters">): T[] {
  const needle = query.search.trim().toLocaleLowerCase();
  const read = columns.filter((col) => !col.actions);
  const active = read.filter((col) => complete(query.filters[col.key]));
  return rows.filter(
    (row) =>
      (needle === "" || read.some((col) => col.text(row).toLocaleLowerCase().includes(needle))) &&
      active.every((col) => passes(col.text(row), col.kind ?? "text", query.filters[col.key]!)),
  );
}

/** A stable sort by one column; empty values last in either direction, as a spreadsheet keeps them. */
export function sortRows<T>(rows: readonly T[], columns: readonly DataColumn<T>[], sort: DataSort | null): T[] {
  const col = sort ? columns.find((each) => each.key === sort.key) : undefined;
  if (!sort || !col) return [...rows];
  const kind = col.kind ?? "text";
  const sign = sort.dir === "asc" ? 1 : -1;
  return rows
    .map((row, at) => ({ row, at, key: ordinal(col.text(row), kind) }))
    .sort((a, b) => {
      if (a.key === null || b.key === null) return a.key === b.key ? a.at - b.at : a.key === null ? 1 : -1;
      const cmp = typeof a.key === "number" && typeof b.key === "number" ? a.key - b.key : String(a.key).localeCompare(String(b.key));
      return cmp !== 0 ? sign * cmp : a.at - b.at;
    })
    .map((each) => each.row);
}

/** One page of the rows the query keeps, and how many it keeps in all. */
export function arrangeRows<T>(rows: readonly T[], columns: readonly DataColumn<T>[], query: DataQuery): { rows: T[]; total: number } {
  const kept = sortRows(filterRows(rows, columns, query), columns, query.sort);
  const offset = Math.min(query.offset, Math.max(0, Math.floor((kept.length - 1) / query.pageSize) * query.pageSize));
  return { rows: kept.slice(offset, offset + query.pageSize), total: kept.length };
}

/** A column's values across every row, most frequent first: the filter's tick list. */
export function valuesOf<T>(rows: readonly T[], column: DataColumn<T>): ColumnValues {
  const counts = new Map<string, number>();
  for (const row of rows) {
    const text = column.text(row);
    counts.set(text, (counts.get(text) ?? 0) + 1);
  }
  const values = [...counts].map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count || a.value.localeCompare(b.value));
  return { values: values.slice(0, MAX_VALUES), scanned: rows.length, total: rows.length, capped: values.length > MAX_VALUES };
}
