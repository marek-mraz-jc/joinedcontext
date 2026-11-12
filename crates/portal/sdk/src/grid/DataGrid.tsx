/**
 * The one grid for a list that is not NGSI-LD (T-6739, owner 2026-11-11: "the datagrid should be
 * shared across frontend, admin UI etc."): a typed array and its columns, with the entity grid's
 * header menu (ColumnMenu), its value filter (AutoFilter), its Arrange columns panel and pager, so
 * an admin list (Policies, Approvals, Registrations) looks and behaves as the data grid does.
 *
 * Search, filters, sort and paging run in the browser over the rows the page read; a host with a
 * server-side list passes `query`, `onQueryChange` and `total`, and the grid shows its rows as given.
 * The grid only narrows what the server returned: it never decides who may read a row.
 */
import React, { useId, useMemo, useState } from "react";
import { AutoFilter } from "./AutoFilter";
import { ArrangePanel } from "./ArrangePanel";
import type { ArrangeItem } from "./ArrangePanel";
import { ColumnMenu } from "./ColumnMenu";
import { moveTo } from "./GridHead";
import { pageNumbers } from "./GridPager";
import { arrangeRows, EMPTY_QUERY, valuesOf } from "./arrayRows";
import type { DataColumn, DataKind, DataQuery } from "./arrayRows";
import { complete } from "./filters";
import type { FilterColumn, FilterKind } from "./filters";
import type { ColumnKind } from "./cells";
import { DEFAULT_LABELS } from "./useEntityGrid";
import type { GridLabels } from "./useEntityGrid";
import "./grid.css";

export interface DataGridLabels extends GridLabels {
  searchRows: string;
  searchRowsHint: string;
  noMatch: string;
  /** "{n} rows", the size of the whole list the query keeps. */
  rowCount: string;
}

export const DEFAULT_DATA_LABELS: DataGridLabels = {
  ...DEFAULT_LABELS,
  searchRows: "Search",
  searchRowsHint: "Type part of any value in the list, e.g. a name.",
  noMatch: "No rows match the search or the filters.",
  rowCount: "{n} rows",
};

/** What a person arranged, kept per `storageKey` in this browser: never data, only the view. */
interface DataLayout {
  order?: string[];
  hidden?: string[];
  shown?: string[];
  labels?: Record<string, string>;
}

export interface DataGridProps<T> {
  /** Names the table for a screen reader. */
  caption: string;
  rows: readonly T[];
  columns: readonly DataColumn<T>[];
  rowKey: (row: T) => string;
  /** The read is under way: the rows are a skeleton. */
  loading?: boolean;
  /** The host's failure panel (the API's reason and a retry): shown instead of the table. */
  error?: React.ReactNode;
  /** The list holds nothing at all: what to do next, such as the page's create action. */
  empty: React.ReactNode;
  labels?: Partial<DataGridLabels>;
  /** Keys the layout a person arranged in this browser (`localStorage`). */
  storageKey?: string;
  pageSizes?: number[];
  /** Below 640 px each row becomes a card of label and value lines. */
  stack?: boolean;
  /** Server-side mode: the host owns the query, answers it and says how many rows it matches. */
  query?: DataQuery;
  onQueryChange?: (query: DataQuery) => void;
  total?: number;
  /** Hides the search box, for a list the page searches itself. */
  noSearch?: boolean;
  /** The host's own controls at the start of the grid's toolbar, such as a filter of its own. */
  tools?: React.ReactNode;
  /** Attributes of the table, such as the host's marker for a table whose rows open a record. */
  tableProps?: React.TableHTMLAttributes<HTMLTableElement> & Record<`data-${string}`, string>;
  /** Attributes of a row, such as the host's click that opens the row's record. */
  rowProps?: (row: T) => React.HTMLAttributes<HTMLTableRowElement>;
}

const COLUMN_KIND: Record<DataKind, ColumnKind> = { text: "text", number: "number", date: "datetime", boolean: "boolean" };
const FILTER_KIND: Record<DataKind, FilterKind> = { text: "text", number: "number", date: "date", boolean: "boolean" };

function readLayout(key: string | undefined): DataLayout {
  if (!key) return {};
  try {
    const raw = window.localStorage.getItem(`jc-data-grid:${key}`);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed) ? (parsed as DataLayout) : {};
  } catch {
    return {};
  }
}

function writeLayout(key: string | undefined, layout: DataLayout): void {
  if (!key) return;
  try {
    window.localStorage.setItem(`jc-data-grid:${key}`, JSON.stringify(layout));
  } catch {
    // Private windows and blocked storage keep the layout for this visit only.
  }
}

export function DataGrid<T>({
  caption,
  rows,
  columns,
  rowKey,
  loading,
  error,
  empty,
  labels: partial,
  storageKey,
  pageSizes = [10, 25, 50, 100],
  stack,
  query: controlled,
  onQueryChange,
  total: remoteTotal,
  noSearch,
  tools,
  tableProps,
  rowProps,
}: DataGridProps<T>): React.JSX.Element {
  const labels = useMemo(() => ({ ...DEFAULT_DATA_LABELS, ...partial }), [partial]);
  const id = useId();
  const [own, setOwn] = useState<DataQuery>({ ...EMPTY_QUERY, pageSize: pageSizes.includes(25) ? 25 : (pageSizes[0] ?? 25) });
  const query = controlled ?? own;
  const setQuery = (next: DataQuery) => (onQueryChange ? onQueryChange(next) : setOwn(next));
  const [layout, setLayoutState] = useState<DataLayout>(() => readLayout(storageKey));
  const setLayout = (next: DataLayout) => {
    setLayoutState(next);
    writeLayout(storageKey, next);
  };
  const [menu, setMenu] = useState<string | null>(null);
  const [filtering, setFiltering] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState<string | null>(null);

  const primary = columns[0]?.key;
  const known = columns.map((col) => col.key);
  // A saved order keeps the columns it names that still exist, then the new ones in the page's order.
  const order = [...(layout.order ?? []).filter((key) => known.includes(key) && key !== primary), ...known.filter((key) => key !== primary && !(layout.order ?? []).includes(key))];
  if (primary !== undefined) order.unshift(primary);
  const isShown = (col: DataColumn<T>) => col.key === primary || (col.hidden ? (layout.shown ?? []).includes(col.key) : !(layout.hidden ?? []).includes(col.key));
  const byKey = new Map(columns.map((col) => [col.key, col]));
  const visible = order.map((key) => byKey.get(key)!).filter(isShown);
  const labelOf = (col: DataColumn<T>) => layout.labels?.[col.key] ?? col.label;
  const show = (key: string, on: boolean) => {
    const col = byKey.get(key);
    if (!col) return;
    if (col.hidden) setLayout({ ...layout, shown: on ? [...(layout.shown ?? []), key] : (layout.shown ?? []).filter((each) => each !== key) });
    else setLayout({ ...layout, hidden: on ? (layout.hidden ?? []).filter((each) => each !== key) : [...(layout.hidden ?? []), key] });
    // A hidden column's filter would narrow the rows with nothing on screen saying so.
    if (!on && query.filters[key]) {
      const { [key]: _gone, ...filters } = query.filters;
      setQuery({ ...query, filters, offset: 0 });
    }
  };

  const remote = onQueryChange !== undefined && remoteTotal !== undefined;
  const page = remote ? { rows: [...rows], total: remoteTotal } : arrangeRows(rows, visible, query);
  const pages = Math.max(1, Math.ceil(page.total / query.pageSize));
  const current = Math.min(pages, Math.floor(query.offset / query.pageSize) + 1);
  const narrowed = query.search.trim() !== "" || Object.values(query.filters).some(complete);
  // A local list that fits the smallest page has nothing to page through: its count alone (T-6752).
  const short = !remote && rows.length <= Math.min(...pageSizes);
  if (error) return <div className="jc-grid jc-data-grid">{error}</div>;

  const items: ArrangeItem[] = order.map((key, at) => {
    const col = byKey.get(key)!;
    // The row's name and its actions are always shown: without them a row cannot be opened.
    const fixed = key === primary || col.actions === true;
    return { key, label: labelOf(col), model: col.label, shown: isShown(col), fixed, canUp: !fixed && at > 1, canDown: !fixed && at < order.length - 1 };
  });

  const header = (col: DataColumn<T>) => {
    const kind = col.kind ?? "text";
    const sorted = query.sort?.key === col.key ? query.sort.dir : null;
    const sortable = !col.actions && col.sortable !== false;
    const filterable = !col.actions && col.filter !== false;
    const fixed = col.key === primary;
    const label = labelOf(col);
    const filterColumn: FilterColumn = { key: col.key, attr: null, kind: FILTER_KIND[kind] };
    return (
      <th
        key={col.key}
        scope="col"
        className={`jc-grid-th${dragOver === col.key ? " jc-grid-th--drop" : ""}${complete(query.filters[col.key]) ? " jc-grid-th--filtered" : ""}`}
        style={col.align === "right" ? { textAlign: "right" } : undefined}
        aria-sort={sorted === "asc" ? "ascending" : sorted === "desc" ? "descending" : undefined}
        draggable={!fixed}
        onDragStart={fixed ? undefined : (e) => { e.dataTransfer.setData("text/x-jc-data-col", col.key); e.dataTransfer.effectAllowed = "move"; }}
        onDragOver={fixed ? undefined : (e) => { if (e.dataTransfer.types.includes("text/x-jc-data-col")) { e.preventDefault(); setDragOver(col.key); } }}
        onDragLeave={fixed ? undefined : () => setDragOver(null)}
        onDrop={fixed ? undefined : (e) => {
          e.preventDefault();
          setDragOver(null);
          const dragged = e.dataTransfer.getData("text/x-jc-data-col");
          if (dragged && dragged !== col.key && dragged !== primary) setLayout({ ...layout, order: moveTo(order, dragged, order.indexOf(col.key)) });
        }}
      >
        {col.heading !== undefined ? (
          <span className="jc-data-grid-heading">
            {col.heading}
            {sorted ? (sorted === "asc" ? " ↑" : " ↓") : ""}
          </span>
        ) : sortable ? (
          <button
            type="button"
            className="jc-grid-sort-btn"
            // Ascending, descending, then the list's own order again.
            onClick={() => setQuery({ ...query, sort: sorted === "desc" ? null : { key: col.key, dir: sorted === "asc" ? "desc" : "asc" }, offset: 0 })}
          >
            {label}
            {sorted ? <span aria-hidden="true">{sorted === "asc" ? " ↑" : " ↓"}</span> : null}
          </button>
        ) : (
          <span>{label}</span>
        )}
        {!col.actions && (
          <span className="jc-grid-th-menu" style={{ position: "relative" }}>
            <button
              type="button"
              className="jc-grid-meta-btn"
              aria-label={`${labels.columnMenu}: ${label}`}
              aria-haspopup="menu"
              aria-expanded={menu === col.key}
              onClick={() => setMenu(menu === col.key ? null : col.key)}
            >
              ⋮
            </button>
            {menu === col.key && (
              <ColumnMenu
                label={label}
                kind={COLUMN_KIND[kind]}
                labels={labels}
                sorted={sorted}
                onSort={(dir) => {
                  setQuery({ ...query, sort: { key: col.key, dir }, offset: 0 });
                  setMenu(null);
                }}
                onFilter={filterable ? () => { setMenu(null); setFiltering(col.key); } : undefined}
                onHide={fixed ? undefined : () => { show(col.key, false); setMenu(null); }}
                onClose={() => setMenu(null)}
              />
            )}
            {filtering === col.key && (
              <AutoFilter
                column={filterColumn}
                label={label}
                labels={labels}
                filter={query.filters[col.key]}
                load={() => Promise.resolve(valuesOf(rows, col))}
                formatValue={(value) => (value === "" ? labels.groupNoValue : (col.valueLabel?.(value) ?? value))}
                onApply={(next) => {
                  const { [col.key]: _old, ...rest } = query.filters;
                  setQuery({ ...query, filters: next ? { ...rest, [col.key]: next } : rest, offset: 0 });
                  setFiltering(null);
                }}
                onClose={() => setFiltering(null)}
              />
            )}
          </span>
        )}
      </th>
    );
  };

  const body = loading ? (
    Array.from({ length: 3 }, (_, r) => (
      <tr key={`skeleton-${r}`} aria-hidden="true">
        {visible.map((col) => (
          <td key={col.key} className="jc-grid-td"><span className="jc-data-grid-skeleton" /></td>
        ))}
      </tr>
    ))
  ) : page.rows.length === 0 ? (
    <tr>
      <td className="jc-grid-empty" colSpan={visible.length}>
        {narrowed ? (
          <span className="jc-data-grid-nomatch">
            {labels.noMatch}{" "}
            <button type="button" className="jc-grid-arrange-btn" onClick={() => setQuery({ ...query, search: "", filters: {}, offset: 0 })}>
              {labels.clearFilters}
            </button>
          </span>
        ) : (
          empty
        )}
      </td>
    </tr>
  ) : (
    page.rows.map((row) => (
      <tr key={rowKey(row)} {...rowProps?.(row)}>
        {visible.map((col, at) => {
          const Cell = at === 0 ? "th" : "td";
          return (
            <Cell
              key={col.key}
              scope={at === 0 ? "row" : undefined}
              className={col.align === "right" ? "jc-grid-td jc-data-grid-end" : "jc-grid-td"}
              data-label={at === 0 || col.actions ? undefined : labelOf(col)}
            >
              {col.cell ? col.cell(row) : col.text(row)}
            </Cell>
          );
        })}
      </tr>
    ))
  );

  return (
    <div className={`jc-grid jc-data-grid${stack ? " jc-data-grid--stack" : ""}`} data-density="comfortable">
      <div className="jc-grid-toolbar jc-data-grid-toolbar">
        {tools}
        {!noSearch && (
          <div className="jc-data-grid-search">
            <label htmlFor={`${id}-search`} className="jc-data-grid-search-label">{labels.searchRows}</label>
            <input
              id={`${id}-search`}
              type="search"
              value={query.search}
              maxLength={200}
              aria-describedby={`${id}-search-hint`}
              onChange={(e) => setQuery({ ...query, search: e.target.value, offset: 0 })}
            />
            <span id={`${id}-search-hint`} className="jc-grid-arrange-hint">{labels.searchRowsHint}</span>
          </div>
        )}
        <ArrangePanel
          labels={labels}
          items={items}
          onShow={show}
          onRename={(key, text) => {
            const { [key]: _old, ...rest } = layout.labels ?? {};
            setLayout({ ...layout, labels: text === undefined ? rest : { ...rest, [key]: text } });
          }}
          onMove={(key, by) => setLayout({ ...layout, order: moveTo(order, key, order.indexOf(key) + by) })}
          onReset={() => setLayout({})}
        />
      </div>
      <div className="jc-grid-scroll" role="group" aria-label={caption} tabIndex={0}>
        <table {...tableProps} className="jc-grid-table" aria-busy={loading ? true : undefined}>
          <caption className="jc-grid-sr-only">{caption}</caption>
          <thead className="jc-grid-thead">
            <tr>{visible.map(header)}</tr>
          </thead>
          <tbody>{body}</tbody>
        </table>
      </div>
      {loading ? (
        <p role="status" className="jc-grid-sr-only">{labels.loading}</p>
      ) : page.total === 0 || (pages === 1 && pageSizes.length === 1) ? null : short ? (
        // One fixed page size and one page: nothing to step or choose (a list the server pages brings its own pager);
        // a short local list shows only its count.
        <div className="jc-grid-pager">
          <span className="jc-grid-total" role="status">{labels.rowCount.replace("{n}", String(page.total))}</span>
        </div>
      ) : (
        <div className="jc-grid-pager">
          <button type="button" className="jc-grid-pager-step" disabled={current <= 1} onClick={() => setQuery({ ...query, offset: (current - 2) * query.pageSize })}>
            <span aria-hidden="true">‹</span> <span className="jc-grid-pager-word">{labels.previous}</span>
          </button>
          <span className="jc-grid-page-info">
            {`${labels.page} ${current}`}
            <span className="jc-grid-page-count">{` / ${pages}`}</span>
          </span>
          {pages > 1 && (
            <span className="jc-grid-pages">
              {pageNumbers(current, pages).map((n, at) =>
                n === null ? (
                  <span key={`gap-${at}`} aria-hidden="true">…</span>
                ) : (
                  <button key={n} type="button" aria-label={`${labels.page} ${n}`} aria-current={n === current ? "page" : undefined} onClick={() => setQuery({ ...query, offset: (n - 1) * query.pageSize })}>
                    {n}
                  </button>
                ),
              )}
            </span>
          )}
          <span className="jc-grid-total" role="status">{labels.rowCount.replace("{n}", String(page.total))}</span>
          <button type="button" className="jc-grid-pager-step" disabled={current >= pages} onClick={() => setQuery({ ...query, offset: current * query.pageSize })}>
            <span className="jc-grid-pager-word">{labels.next}</span> <span aria-hidden="true">›</span>
          </button>
          <label className="jc-grid-page-size">
            {labels.rowsPerPage}
            <select value={query.pageSize} onChange={(e) => setQuery({ ...query, pageSize: Number(e.target.value), offset: 0 })}>
              {pageSizes.map((size) => (
                <option key={size} value={size}>{size}</option>
              ))}
            </select>
          </label>
        </div>
      )}
    </div>
  );
}
