import React from "react";
import { HeaderFilter } from "./AutoFilter";
import { ColumnMenu } from "./ColumnMenu";
import { complete } from "./filters";
import { GridFilterRow } from "./GridFilterRow";
import { KIND_ICON, KIND_LABEL } from "./gridKinds";
import { cellText } from "./model";
import { NAME_COLUMN } from "./useEntityGrid";
import type { MetaKey, VisibleColumn } from "./useEntityGrid";
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
  const { grid, labels, kindOf, order, asText, filterable, dragOver, setDragOver, layout, setLayout, sortLabel, where, openFilter, closeFilter, focusFilter, loadValues, locks, openMenu, toggleMenu, menuRef, baseLabel, closeMenu, hideColumn, renameColumn, widen, firstMovable, startResize } = view;
  const { state, setFilter, setSort, getHeaderProps } = grid;
  const kind = kindOf.get(col.key) ?? "text";
  const kindLabel = labels[KIND_LABEL[kind]] as string;
  const own = col.attr !== null && col.meta === null ? col.attr : null;
  const at = own === null ? -1 : order.indexOf(own);
  const fixed = own === NAME_COLUMN;
  const movable = own !== null && !col.pinned && !fixed;
  // The Type filter lives in the filter row alone: its values are the grid's own types.
  const headerFilter = !asText && filterable.get(col.key)?.kind !== "type" ? filterable.get(col.key) : undefined;
  return (
  <th
      className={`jc-grid-th${col.pinned ? " jc-grid-pinned" : ""}${dragOver === col.key ? " jc-grid-th--drop" : ""}${complete(state.filters[col.key]) ? " jc-grid-th--filtered" : ""}`}
    // No header is squeezed below its first word (T-6328): a set width is the person's own.
    style={col.width ? { width: col.width, minWidth: col.width, maxWidth: col.width } : { minWidth: firstWordWidth(col.label) }}
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
    {locks.get(col.key) !== undefined ? (
      <span className="jc-grid-lock" role="img" aria-label={`${labels.locked}: ${locks.get(col.key)}`} title={locks.get(col.key)} />
    ) : null}
    {own !== null && (
      <button
        type="button"
        className="jc-grid-meta-btn"
        aria-label={`${labels.columnMenu}: ${col.label}`}
        aria-haspopup="dialog"
        aria-expanded={openMenu === own}
        onClick={() => toggleMenu(own)}
      >
        ⋮
      </button>
    )}
    {own !== null && openMenu === own && (
      <ColumnMenu
        ref={menuRef}
        label={layout.labels?.[own] ?? baseLabel(col)}
        labels={labels}
        sorted={state.sort?.attr === own ? state.sort.dir : null}
        onSort={(dir) => { setSort(own, dir); closeMenu(); }}
        onFilter={headerFilter ? () => focusFilter(col.key) : undefined}
        onHide={col.pinned || fixed ? undefined : () => hideColumn(own)}
        renamed={layout.labels?.[own] !== undefined}
        onRename={(text) => renameColumn(own, text)}
        onWiden={(delta) => widen(own, col.width, delta)}
        onMove={movable ? (delta) => setLayout({ ...layout, order: moveTo(order, own, at + delta) }) : undefined}
        canMoveLeft={movable && at > firstMovable}
        canMoveRight={movable && at < order.length - 1}
        onClose={closeMenu}
          details={<ColumnDetails view={view} col={col} />}
        />
      )}
    {own !== null && (
      // The edge a pointer drags; a keyboard has Narrower and Wider under the menu's settings.
      <span className="jc-grid-resize" aria-hidden="true" onPointerDown={(e) => startResize(e, own)} />
    )}
  </th>
  );
}

/** Under a column menu's Details: the attribute's history and the metadata columns it can show. */
function ColumnDetails({ view, col }: { view: GridView; col: VisibleColumn }): React.JSX.Element {
  const { grid, labels, options, setHistory, setOpenMenu, handleMetaCheck } = view;
  const { rows, state } = grid;
  return (
    <>
    {options.source.history && options.config.history.enabled !== false && (
      <button
        type="button"
        className="jc-grid-meta-item"
        onClick={() => {
          // The row the person is on, else the first: history belongs to one
          // entity, and the panel names which.
          const row = rows[state.activeCell?.row ?? 0] ?? rows[0];
          // Named as a person reads it: the column's label and the row's name,
          // never the id (T-2991).
          const name = cellText(row?.cells.name);
          const title = options.labels?.historyLabels?.title ?? labels.history;
          setHistory({
            attr: col.attr!,
            id: row?.id ?? "",
            heading: `${title}: ${col.label}${name ? ` — ${name}` : ""}`,
          });
          setOpenMenu(null);
        }}
      >
        {labels.history}
      </button>
    )}
    {(["observedAt", "unit", "datasetId", "createdAt", "modifiedAt"] as MetaKey[]).map((meta) => {
      const checked = (state.shown[col.attr!] ?? []).includes(meta);
      const metaLabel = meta === "observedAt" ? labels.observedAt : meta === "unit" ? labels.unit : meta === "datasetId" ? labels.datasetId : meta === "createdAt" ? labels.createdAt : labels.modifiedAt;
      return (
        <label key={meta} className="jc-grid-meta-item">
          <input
            type="checkbox"
            checked={checked}
            onChange={() => handleMetaCheck(col.attr!, meta)}
          />
          {metaLabel}
        </label>
      );
    })}
    </>
  );
}
