import React, { useCallback, useRef } from "react";
import { coerce, storedText } from "./GridCellEditors";
import { parseClipboard, planPaste } from "./paste";
import type { ValueRule } from "./rules";
import type { AttributeAccess } from "./source";
import type { EntityGrid as EntityGridApi, UseEntityGridOptions, VisibleColumn } from "./useEntityGrid";

/** The text boxes a typed character starts editing; a date or a picker opens with Enter instead. */
const TYPED_INPUTS = new Set(["text", "number", "url", "search"]);

/**
 * The grid's keyboard, focus and paste. The table is the grid's one tab stop: Enter or F2 goes into
 * the active cell's own control (its input, list or picker), Escape comes back to the grid, and the
 * active cell is kept in view.
 */
export function useGridKeys(
  gridOptions: UseEntityGridOptions,
  grid: EntityGridApi,
  {
    editing,
    editable,
    inspects,
    locks,
    rules,
    accessRows,
    accessOf,
    setInspected,
    setPasteNote,
  }: {
    editing: boolean;
    editable: (column: VisibleColumn) => boolean;
    inspects: (column: VisibleColumn) => column is VisibleColumn & { attr: string };
    locks: Map<string, string | undefined>;
    rules?: Record<string, ValueRule>;
    accessRows?: AttributeAccess[] | null;
    accessOf: (attr: string) => AttributeAccess | undefined;
    setInspected: (panel: { attr: string; label: string; row?: EntityGridApi["rows"][number] }) => void;
    setPasteNote: (note: string | null) => void;
  },
) {
  const { rows, columns, state, labels, getGridProps, setEdit, setEdits } = grid;
  const relations = gridOptions.relations;
  const gridProps = getGridProps() as Record<string, unknown> & { onKeyDown: (e: React.KeyboardEvent) => void };
  const activeCellId = gridProps["aria-activedescendant"] as string | undefined;
  React.useEffect(() => {
    if (!activeCellId) return;
    document.getElementById(activeCellId)?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [activeCellId]);
  // The cell being edited and its pending value when editing began, so Escape puts that back.
  const entered = useRef<{ id: string; attr: string; before: unknown } | null>(null);
  const cellAt = useCallback(
    (row: number, col: number) => {
      const id = rows[row]?.id;
      const column = columns[col];
      return id !== undefined && column && editable(column) && column.attr !== null ? { id, attr: column.attr } : null;
    },
    [rows, columns, editable],
  );
  const onGridFocus = useCallback(
    (e: React.FocusEvent<HTMLTableElement>) => {
      if (e.target === e.currentTarget) return;
      // A cell's own id ends in -r{row}-c{col}: the control focused, by key or click, is that cell's.
      const where = /-r(\d+)-c(\d+)$/.exec(e.target.closest('[role="gridcell"]')?.id ?? "");
      if (!where) return;
      // Tab goes from cell to cell in the page's own order and out of the grid after the last, so
      // the active cell follows the focus rather than holding the key (WCAG 2.1.2).
      const [row, col] = [Number(where[1]), Number(where[2])];
      if (state.activeCell?.row !== row || state.activeCell.col !== col) grid.setActive(row, col);
      const cell = cellAt(row, col);
      if (cell && (entered.current?.id !== cell.id || entered.current.attr !== cell.attr)) {
        entered.current = { ...cell, before: state.edits[cell.id]?.[cell.attr] };
      }
    },
    [cellAt, state.edits, state.activeCell, grid],
  );
  /** Leaves a cell's control for the grid; Enter then goes one row down, as in a spreadsheet. */
  const leaveCell = useCallback(
    (table: HTMLTableElement, key: string) => {
      entered.current = null;
      table.focus();
      if (key === "Enter") grid.moveActive("ArrowDown");
    },
    [grid],
  );
  const onGridKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTableElement>) => {
      const onGrid = e.target === e.currentTarget;
      if (!onGrid) {
        const control = e.target as HTMLElement;
        if (e.key === "Escape") {
          // Escape cancels what was typed since the cell was entered, then comes back to the grid.
          e.preventDefault();
          const was = entered.current;
          if (was) setEdit(was.id, was.attr, was.before);
          leaveCell(e.currentTarget, "Escape");
          return;
        }
        // In a cell's text box, Enter keeps the value and goes on, as in a spreadsheet; a picker's
        // own button or list keeps its keys.
        if (control.tagName === "INPUT" && control.closest('[role="gridcell"]') && e.key === "Enter") {
          e.preventDefault();
          leaveCell(e.currentTarget, e.key);
          return;
        }
      }
      const control = activeCellId
        ? document.getElementById(activeCellId)?.querySelector<HTMLElement>(".jc-grid-cell input, .jc-grid-cell select, .jc-grid-cell textarea, .jc-grid-cell button")
        : null;
      if (onGrid && (e.key === "Enter" || e.key === "F2") && control) {
        e.preventDefault();
        control.focus();
        return;
      }
      // `i` on a cell that opens no editor shows what the person may do with it and why (T-5528);
      // on an editable cell it is typed, as in every spreadsheet, and the cell's "i" button opens it.
      const here = state.activeCell ? columns[state.activeCell.col] : undefined;
      if (onGrid && e.key === "i" && here && inspects(here) && !editable(here)) {
        e.preventDefault();
        setInspected({ attr: here.attr, label: here.label, row: rows[state.activeCell!.row] });
        return;
      }
      // Typing on an editable cell starts editing it with that character, as Baserow and every
      // spreadsheet do: the value is replaced, Escape brings it back.
      const typed = e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey;
      const at = state.activeCell;
      const cell = at ? cellAt(at.row, at.col) : null;
      if (onGrid && typed && cell && control instanceof HTMLInputElement && TYPED_INPUTS.has(control.type)) {
        e.preventDefault();
        control.focus();
        const stored = storedText(rows[at!.row]?.cells[cell.attr]);
        setEdit(cell.id, cell.attr, e.key === stored ? undefined : coerce(e.key, stored, rules?.[cell.attr]));
        return;
      }
      // Typing on a locked cell says why nothing opens (T-5530), in the grid's status line.
      const lockedHere = here ? locks.get(here.key) : undefined;
      if (onGrid && typed && lockedHere !== undefined) {
        setPasteNote(`${labels.locked}: ${lockedHere}`);
      }
      gridProps.onKeyDown(e);
    },
    [activeCellId, gridProps, setEdit, leaveCell, state.activeCell, cellAt, rows, rules, columns, inspects, editable, locks, labels.locked],
  );

  /**
   * A range copied from a spreadsheet, laid over the grid from the active cell in edit mode
   * (T-3097). Only editable, typed cells of listed rows take a value, an enum only a value it lists;
   * every other cell is skipped and counted. What lands is pending like any edit.
   */
  const onGridPaste = useCallback(
    (e: React.ClipboardEvent<HTMLTableElement>) => {
      if (!editing || e.target !== e.currentTarget) return;
      const text = e.clipboardData.getData("text/plain");
      if (!text) return;
      e.preventDefault();
      const start = state.activeCell ?? { row: 0, col: 0 };
      const plan = planPaste(parseClipboard(text), (r, c) => {
        const row = rows[start.row + r];
        const column = columns[start.col + c];
        // The server's own refusal is counted apart, so the note says why those cells did not change.
        if (row && column && accessRows && column.attr !== null && column.meta === null && column.key !== "id" && accessOf(column.attr)?.write !== true) {
          return { id: row.id, attr: undefined, current: null, refused: true };
        }
        if (!row || !column || !editable(column) || column.attr === null) {
          return { id: row?.id, attr: undefined, current: null };
        }
        const cell = row.cells[column.attr];
        const one = Array.isArray(cell) ? cell[0] : cell;
        // A geometry is drawn on the map and a relationship picked: neither is typed over.
        if (one?.kind === "geo" || one?.kind === "relationship" || relations?.[column.attr]) {
          return { id: row.id, attr: column.attr, current: null };
        }
        const pending = state.edits[row.id]?.[column.attr];
        const stored = one?.value === undefined || one?.value === null ? "" : String(one.value);
        const options = gridOptions.enums?.[column.attr];
        return {
          id: row.id,
          attr: column.attr,
          current: pending === undefined ? stored : String(pending),
          allowed: options && options.length > 0 ? options.map((option) => option.value) : undefined,
        };
      });
      if (plan.tooLarge) {
        setPasteNote(labels.pasteTooLarge);
        return;
      }
      setEdits(
        plan.edits.map((edit) => {
          const cell = rows.find((row) => row.id === edit.id)?.cells[edit.attr];
          const one = Array.isArray(cell) ? cell[0] : cell;
          const stored = one?.value === undefined || one?.value === null ? "" : String(one.value);
          return {
            id: edit.id,
            attr: edit.attr,
            value: edit.text === stored ? undefined : coerce(edit.text, stored, rules?.[edit.attr]),
          };
        }),
      );
      setPasteNote(
        `${plan.edits.length} ${labels.pasted}${plan.refused > 0 ? `, ${plan.refused} ${labels.notAllowed}` : ""}${plan.skipped > 0 ? `, ${plan.skipped} ${labels.skipped}` : ""}`,
      );
    },
    [editing, state.activeCell, state.edits, rows, columns, editable, relations, gridOptions.enums, labels.pasteTooLarge, labels.pasted, labels.skipped, labels.notAllowed, setEdits, rules, accessRows, accessOf],
  );
  return { gridProps, onGridKeyDown, onGridFocus, onGridPaste };
}
