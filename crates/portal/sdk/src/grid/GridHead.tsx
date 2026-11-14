import React, { useLayoutEffect, useRef, useState } from "react";
import { HeaderFilter } from "./AutoFilter";
import { ColumnMenu, WIDTH_STEP } from "./ColumnMenu";
import { complete } from "./filters";
import { GridFilterRow } from "./GridFilterRow";
import { KIND_ICON, KIND_LABEL } from "./gridKinds";
import { MAX_COLUMN_WIDTH, MIN_COLUMN_WIDTH, NAME_COLUMN } from "./useEntityGrid";
import type { VisibleColumn } from "./useEntityGrid";
import type { GridView } from "./useGridView";

/** `list` with `attr` moved to `index`, every other name in its order. */
export function moveTo(list: readonly string[], attr: string, index: number): string[] {
  const rest = list.filter((each) => each !== attr);
  const at = Math.max(0, Math.min(index, rest.length));
  return [...rest.slice(0, at), attr, ...rest.slice(at)];
}

const DRAG_TYPE = "application/x-jc-grid-column";

function firstWordWidth(label: string): string {
  const word = label.trim().split(/\s+/)[0] ?? "";
  return `calc(${Math.min(word.length, 16)}ch + 4.5rem)`;
}


/** The table's head: one header per column with its sort, filter and menu, and the filter row under them. */
export function GridHead({ view }: { view: GridView }): React.JSX.Element {
  const { props, grid, allChecked } = view;
  const { rows, columns } = grid;
  const { classNames, rowNumbers, onSelectedChange } = props;
  return (
    <thead className={`jc-grid-thead${classNames?.header ? ` ${classNames.header}` : ""}`}>
      <tr>
        {rowNumbers && (
          <th className="jc-grid-rownum" scope="col">
            {/* The label is the finger's target: 44 px on a touch screen, the box keeps its size (T-5596). */}
            <label className="jc-grid-check">
              <input
                type="checkbox"
                aria-label={rowNumbers.selectAll}
                checked={allChecked}
                disabled={!onSelectedChange || rows.length === 0}
                onChange={() => onSelectedChange?.(allChecked ? [] : rows.map((row) => row.id))}
              />
            </label>
          </th>
        )}
          {columns.map((col, i) => (
            <ColumnHeader key={col.key} view={view} col={col} index={i} />
          ))}
        </tr>
        <GridFilterRow view={view} />
      </thead>
  );
}

function ColumnHeader({ view, col, index: i }: { view: GridView; col: VisibleColumn; index: number }): React.JSX.Element {
  const { grid, labels, kindOf, order, asText, filterable, dragOver, setDragOver, layout, setLayout, sortLabel, where, openFilter, closeFilter, focusFilter, loadValues, locks, openMenu, toggleMenu, menuRef, closeMenu, hideColumn, widen, startResize, inspects, setInspected, inspectorLabels } = view;
  const { state, setFilter, setSort, getHeaderProps } = grid;
  const kind = kindOf.get(col.key) ?? "text";
  const kindLabel = labels[KIND_LABEL[kind]] as string;
  const own = col.attr !== null && col.meta === null ? col.attr : null;
  const fixed = own === NAME_COLUMN;
  const movable = own !== null && !col.pinned && !fixed;
  // The Type filter lives in the filter row alone: its values are the grid's own types.
  const headerFilter = !asText && filterable.get(col.key)?.kind !== "type" ? filterable.get(col.key) : undefined;
  // A focusable separator states its value: the set width, else the width the browser gave the column.
  const th = useRef<HTMLTableCellElement>(null);
  const [laidOut, setLaidOut] = useState(MIN_COLUMN_WIDTH);
  useLayoutEffect(() => {
    if (!col.width && th.current) setLaidOut(Math.max(MIN_COLUMN_WIDTH, th.current.offsetWidth));
  }, [col.width]);
  return (
  <th
    ref={th}
      className={`jc-grid-th${col.pinned ? " jc-grid-pinned" : ""}${dragOver === col.key ? " jc-grid-th--drop" : ""}${complete(state.filters[col.key]) ? " jc-grid-th--filtered" : ""}`}
    // No header is squeezed below its first word (T-6328): a set width is the person's own.
    // A compact grid's label is as wide as a phone allows (grid.css), whatever width the model gave it (T-6748).
    style={view.props.compact && col.pinned ? undefined : col.width ? { width: col.width, minWidth: col.width, maxWidth: col.width } : { minWidth: firstWordWidth(col.label) }}
    data-col-attr={own ?? undefined}
    draggable={movable}
    onDragStart={movable ? (e) => { e.dataTransfer.setData(DRAG_TYPE, own!); e.dataTransfer.effectAllowed = "move"; } : undefined}
    onDragOver={movable ? (e) => { if (e.dataTransfer.types.includes(DRAG_TYPE)) { e.preventDefault(); setDragOver(col.key); } } : undefined}
    onDragLeave={movable ? () => setDragOver(null) : undefined}
    onDrop={movable ? (e) => {
      e.preventDefault();
      setDragOver(null);
      const dragged = e.dataTransfer.getData(DRAG_TYPE);
      if (dragged && dragged !== own) setLayout({ ...layout, order: moveTo(order, dragged, order.indexOf(own!)) });
    } : undefined}
    {...getHeaderProps(col, i)}
  >
    {/* The glyph is drawn by CSS from data-icon, so the header's text stays its label. */}
    <span className="jc-grid-kind" data-icon={KIND_ICON[kind]} data-kind={kind} role="img" aria-label={kindLabel} title={kindLabel} />
    <button
      type="button"
      className="jc-grid-sort-btn"
      // A source that orders the whole answer sorts every row; elsewhere the order
      // is of the loaded page, and the label never promises the whole set (UI-66).
      title={`${sortLabel(col)} ${col.label}`}
      aria-label={`${sortLabel(col)} ${col.label}`}
      onClick={() => setSort(col.attr ?? col.key)}
    >
      {col.label}
      {state.sort?.attr === (col.attr ?? col.key) ? (state.sort.dir === "asc" ? " ↑" : " ↓") : ""}
    </button>
    {headerFilter && (
      <HeaderFilter
        column={headerFilter}
        label={col.label}
        labels={labels}
        locale={where}
        filter={state.filters[col.key]}
        open={openFilter === col.key}
        onOpen={() => focusFilter(col.key)}
        onClose={closeFilter}
        load={loadValues(headerFilter)}
        onApply={(next) => {
          setFilter(col.key, next);
          closeFilter();
        }}
      />
    )}
    {view.historyMarked(col) ? (
      // A button, so a keyboard and a screen reader reach the history the label promises (T-6878).
      <button
        type="button"
        className="jc-grid-history-mark"
        aria-label={`${labels.historyAvailable}: ${col.label}`}
        title={labels.historyAvailable}
        onClick={() => openHistory(view, col)}
      />
    ) : null}
    {locks.get(col.key) !== undefined ? (
      <span className="jc-grid-lock" role="img" aria-label={`${labels.locked}: ${locks.get(col.key)}`} title={locks.get(col.key)} />
    ) : null}
    {own !== null && (
      <button
        type="button"
        className="jc-grid-meta-btn"
        aria-label={`${labels.columnMenu}: ${col.label}`}
        aria-haspopup="menu"
        aria-expanded={openMenu === own}
        onClick={() => toggleMenu(own)}
      >
        ⋮
      </button>
    )}
    {own !== null && openMenu === own && (
      <ColumnMenu
        ref={menuRef}
        label={col.label}
        kind={kind}
        labels={labels}
        sorted={state.sort?.attr === own ? state.sort.dir : null}
        onSort={(dir) => { setSort(own, dir); closeMenu(); }}
        onFilter={headerFilter ? () => focusFilter(col.key) : undefined}
        onHide={col.pinned || fixed ? undefined : () => hideColumn(own)}
        onHistory={view.historyOffered(col) ? () => openHistory(view, { ...col, attr: own }) : undefined}
        // A phone's way to the access panel its cells no longer carry (T-6781).
        onInspect={view.props.compact && inspects(col) ? { label: inspectorLabels.title, open: () => { closeMenu(); setInspected({ attr: own, label: col.label }); } } : undefined}
        onClose={closeMenu}
      />
    )}
    {own !== null && (
      // The edge a pointer drags; a keyboard focuses it and widens or narrows with the arrow keys.
      <span
        className="jc-grid-resize"
        role="separator"
        aria-orientation="vertical"
        aria-label={`${labels.columnWidth}: ${col.label}`}
        aria-valuenow={col.width ?? laidOut}
        aria-valuemin={MIN_COLUMN_WIDTH}
        aria-valuemax={MAX_COLUMN_WIDTH}
        tabIndex={0}
        onFocus={() => {
          if (!col.width && th.current) setLaidOut(Math.max(MIN_COLUMN_WIDTH, th.current.offsetWidth));
        }}
        onPointerDown={(e) => startResize(e, own)}
        onKeyDown={(e) => {
          if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
          e.preventDefault();
          e.stopPropagation();
          widen(own, col.width, e.key === "ArrowRight" ? WIDTH_STEP : -WIDTH_STEP);
        }}
      />
    )}
  </th>
  );
}

/** A column's history for the row the person is on, under the grid. */
function openHistory(view: GridView, col: VisibleColumn & { attr: string }): void {
  const { rows, state } = view.grid;
  // The row the person is on, else the first: history belongs to one entity, and the panel names which.
  view.openHistoryOf(col, rows[state.activeCell?.row ?? 0] ?? rows[0]);
  view.setOpenMenu(null);
}
