import React from "react";
import { cellText } from "./model";
import type { RichCell, RichRow } from "./model";
import { unitTitle } from "../sdk/units";
import type { VisibleColumn } from "./useEntityGrid";
import type { GridView } from "./useGridView";

/** One group header of a grid grouped by `groupBy` (T-5188): its level and the values it is in. */
export interface GridGroup {
  /** 0 for the outermost group. */
  level: number;
  attr: string;
  /** The values of this group and the groups around it, outermost first; `undefined` for an absent value. */
  path: { attr: string; value: unknown }[];
}

function unitHover(cell: RichCell | RichCell[] | undefined, meta: string | null): string | undefined {
  if (meta || !cell || Array.isArray(cell) || cell.kind !== "property" || !cell.unitCode) return undefined;
  return unitTitle(cell.unitCode);
}

/** The group headers before a row (T-5188): a group starts where its value differs from the row before. */
function groupsBefore(rows: RichRow[], groupBy: readonly string[], row: RichRow, rowIndex: number): GridGroup[] {
  const before = rowIndex > 0 ? rows[rowIndex - 1] : undefined;
  const path = groupBy.map((attr) => {
    const cell = row.cells[attr];
    const one = Array.isArray(cell) ? cell[0] : cell;
    return { attr, value: one?.kind === "relationship" ? one.object : one?.value };
  });
  const first = before === undefined ? 0 : groupBy.findIndex((attr) => cellText(before.cells[attr]) !== cellText(row.cells[attr]));
  return first < 0 ? [] : path.slice(first).map((step, at) => ({ level: first + at, attr: step.attr, path: path.slice(0, first + at + 1) }));
}

/** The table's body: the rows in sight between their spacers, each group's header before its first row. */
export function GridRows({ view }: { view: GridView }): React.JSX.Element {
  const { props, options, grid, shownRows, shownFrom, spacerAbove, spacerBelow, lead, labels } = view;
  const { rows, columns } = grid;
  const { classNames, groupHeader } = props;
  // The rows come ordered by the group keys, so each group's rows are together.
  const groupBy = options.groupBy ?? [];
  return (
    <tbody className={`jc-grid-tbody${classNames?.row ? ` ${classNames.row}` : ""}`}>
      {spacerAbove > 0 && (
        <tr className="jc-grid-spacer" aria-hidden="true">
          <td colSpan={columns.length + lead} style={{ height: spacerAbove }} />
        </tr>
      )}
      {shownRows.map((row, shownIndex) => {
        const rowIndex = shownFrom + shownIndex;
        return (
          <React.Fragment key={row.id}>
          {groupsBefore(rows, groupBy, row, rowIndex).map((group) => {
            const column = columns.find((col) => col.attr === group.attr && col.meta === null);
            const text = cellText(row.cells[group.attr]);
            return (
              <tr key={`group-${group.level}`} className="jc-grid-group" data-level={group.level}>
                <th scope="colgroup" colSpan={columns.length + lead} style={{ paddingInlineStart: `${0.75 + group.level * 1.25}rem` }}>
                  <span className="jc-grid-group-name">{column?.label ?? group.attr}</span>{" "}
                  <span className="jc-grid-group-value">{text === "" ? labels.groupNoValue : text}</span>
                  {groupHeader ? <span className="jc-grid-group-extra">{groupHeader(group)}</span> : null}
                </th>
              </tr>
            );
          })}
            <GridRow view={view} row={row} rowIndex={rowIndex} />
          </React.Fragment>
        );
      })}
      {spacerBelow > 0 && (
        <tr className="jc-grid-spacer" aria-hidden="true">
          <td colSpan={columns.length + lead} style={{ height: spacerBelow }} />
        </tr>
      )}
    </tbody>
  );
}

/** One row: its number and checkbox where the host asked, a swatch for its tone, and one cell per column. */
function GridRow({ view, row, rowIndex }: { view: GridView; row: RichRow; rowIndex: number }): React.JSX.Element {
  const { props, grid, refusedOf, rowTitle, rowName, checkRow, openDetail } = view;
  const { columns, getRowProps } = grid;
  const { classNames, marks, rowNumbers, rowTone, selected = [], onSelectedChange } = props;
  const labels = view.labels;
  const tone = rowTone?.(row);
  return (
    <tr
      className={`jc-grid-tr${tone ? ` jc-grid-tr--${tone.tone}` : ""}${classNames?.row ? ` ${classNames.row}` : ""}`}
      // A refused update stays visible on its own row, not only in the panel the person
      // may have closed: the value there is still theirs and still unapplied.
      data-refused={refusedOf.has(row.id) ? "true" : undefined}
      data-mark={marks?.rows?.[row.id] ? "row" : undefined}
      title={marks?.rows?.[row.id] ?? rowTitle(row.id)}
      {...getRowProps(row, rowIndex)}
      aria-selected={rowNumbers ? selected.includes(row.id) : undefined}
    >
      {rowNumbers && (
        <td className="jc-grid-rownum">
          {/* A tap beside the box ticks it and does not open the row. */}
          <label className="jc-grid-check" onClick={(e) => e.stopPropagation()}>
            <input
              type="checkbox"
              aria-label={`${rowNumbers.selectRow}: ${rowName(row)}`}
              checked={selected.includes(row.id)}
              disabled={!onSelectedChange}
              onClick={(e) => {
                e.stopPropagation();
                checkRow(rowIndex, row.id, e.shiftKey);
              }}
              onChange={() => undefined}
            />
          </label>
          {/* A pointer's shortcut: the row's name is the same button for a keyboard and a screen reader. */}
          <button
            type="button"
            className="jc-grid-expand"
            tabIndex={-1}
            aria-hidden="true"
            title={labels.openRow}
            onClick={(e) => {
              e.stopPropagation();
              openDetail(row);
            }}
          >
            ⤢
          </button>
        </td>
      )}
      {columns.map((col, colIndex) => (
        <GridCell key={`${row.id}-${col.key}`} view={view} row={row} rowIndex={rowIndex} col={col} colIndex={colIndex} tone={tone} />
      ))}
    </tr>
  );
}

/** One cell: a click opens its row on the attribute, the "i" opens what the person may do with it. */
function GridCell({ view, row, rowIndex, col, colIndex, tone }: { view: GridView; row: RichRow; rowIndex: number; col: VisibleColumn; colIndex: number; tone?: { tone: string; label: string } }): React.JSX.Element {
  const { props, grid, kindOf, locks, inspects, editable, selection, setCellPlace, setDetailId, setInspected, inspectorLabels, formatOf, renderCellContent } = view;
  const { getCellProps, cellOf } = grid;
  const { classNames, marks } = props;
  return (
  <td
    data-col={col.attr ? undefined : col.key}
    data-label={col.label}
    data-kind={col.pinned ? undefined : kindOf.get(col.key)}
    className={`jc-grid-td${col.pinned ? " jc-grid-pinned" : ""}${locks.get(col.key) !== undefined ? " jc-grid-td--locked" : ""}${classNames?.cell ? ` ${classNames.cell}` : ""}`}
    {...getCellProps(row, rowIndex, col, colIndex)}
    onClick={
      (inspects(col) || (col.attr && !col.meta && !selection.inShell)) && !editable(col)
        ? (e: React.MouseEvent) => {
            // A control of the cell (a chip, a link) keeps its own click.
            if ((e.target as HTMLElement).closest("button, a, input, select")) return;
            if (col.attr && !col.meta && !selection.inShell) {
              setCellPlace({ tab: "attribute", attr: col.attr });
              setDetailId(row.id);
              return;
            }
            if (col.attr) setInspected({ attr: col.attr, label: col.label, row });
          }
        : undefined
    }
    data-mark={col.attr && marks?.columns?.[col.attr] ? "column" : undefined}
    title={
      (col.attr ? marks?.columns?.[col.attr] : undefined) ??
      (col.pinned ? cellOf(row, col).text : undefined) ??
      unitHover(cellOf(row, col).cell, col.meta)
    }
  >
    {colIndex === 0 && tone ? (
      <span className={`jc-grid-tone jc-grid-tone--${tone.tone}`} role="img" aria-label={tone.label} title={tone.label} />
    ) : null}
    {inspects(col) ? (
      <button
        type="button"
        className="jc-grid-info"
        tabIndex={-1}
        aria-label={`${inspectorLabels.title}: ${col.label}`}
        title={`${inspectorLabels.title}: ${col.label}`}
        onClick={(e) => {
          e.stopPropagation();
          setInspected({ attr: col.attr, label: col.label, row });
        }}
      />
    ) : null}
    {/* At most three lines at the column's widest (T-5192): a dragged width replaces
        the cap, the full value stays in the row's detail. */}
    <div className="jc-grid-cell" data-format={col.attr ? formatOf.get(col.attr) : undefined} style={col.width ? { maxWidth: "none" } : undefined}>
      {renderCellContent(row, col)}
    </div>
  </td>
  );
}

/** The footer row, where the host says what a column's foot shows. */
export function GridFooter({ view }: { view: GridView }): React.JSX.Element | null {
  const { props, grid } = view;
  const { footer, rowNumbers } = props;
  const { rows, columns } = grid;
  if (!footer || rows.length === 0) return null;
  return (
    <tfoot className="jc-grid-tfoot">
      <tr>
        {rowNumbers && <td className="jc-grid-rownum" />}
        {columns.map((col) => (
          <td key={`foot-${col.key}`} className={`jc-grid-foot${col.pinned ? " jc-grid-pinned" : ""}`}>
            {footer(col)}
          </td>
        ))}
      </tr>
    </tfoot>
  );
}
