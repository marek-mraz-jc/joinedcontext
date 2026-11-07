import React from "react";
import { GridHead } from "./GridHead";
import { GridFooter, GridRows } from "./GridRows";
import type { GridView } from "./useGridView";

/**
 * The table alone: head, filter row, body and footer, with the grid's keyboard and paste. A host
 * renders it inside `GridFrame`, with whichever other parts it wants around it.
 */
export function GridTable({ view }: { view: GridView }): React.JSX.Element {
  const { props, grid, scrollRef, virtual, onScroll, fitHeight, tableRef, gridProps, onGridKeyDown, onGridFocus, onGridPaste } = view;
  const { rows, loading } = grid;
  const { classNames } = props;
  return (
  <div
    className="jc-grid-scroll"
    ref={scrollRef}
    onScroll={virtual ? onScroll : undefined}
    style={virtual && fitHeight !== undefined ? { maxHeight: fitHeight } : undefined}
  >
    <table
      ref={tableRef}
      className={`jc-grid-table${classNames?.table ? ` ${classNames.table}` : ""}`}
      {...gridProps}
      // The rows of the last answer stay, dimmed, while the next question is read (T-5623).
      aria-busy={loading && rows.length > 0 ? true : undefined}
      onKeyDown={onGridKeyDown}
      onFocus={onGridFocus}
      onPaste={onGridPaste}
    >
          <GridHead view={view} />
          <GridRows view={view} />
          <GridFooter view={view} />
        </table>
      </div>
  );
}
