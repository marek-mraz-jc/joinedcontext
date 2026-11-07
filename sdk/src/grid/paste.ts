/**
 * Pasting a range into the grid in edit mode (T-3097): the tab-separated text a spreadsheet copies,
 * laid over the grid from its active cell. Pure, so what a paste would change is known before any
 * of it is pending.
 */

/**
 * Cells one paste fills at most: a spreadsheet's whole sheet is not a correction, and each cell
 * becomes a write the gateway checks against its model (DM-86). A larger load is an import.
 */
export const MAX_PASTE_CELLS = 500;

/** The rows and cells of copied text: tab between cells, a line break between rows. */
export function parseClipboard(text: string): string[][] {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  // A spreadsheet ends its copy with a line break; that is not an empty last row.
  if (lines.length > 1 && lines[lines.length - 1] === "") {
    lines.pop();
  }
  return lines.map((line) => line.split("\t"));
}

/** One target cell as the plan needs it. */
export interface PasteTarget {
  /** The entity, or nothing past the page's last row. */
  id: string | undefined;
  /** The attribute, or nothing when the column is not one the person may edit. */
  attr: string | undefined;
  /** What the cell holds now, as its editor shows it; `null` when it cannot be typed over. */
  current: string | null;
  /** The values an enum allows, when the attribute has one. */
  allowed?: readonly string[];
  /** The server refuses the person a write of this attribute (T-5530): skipped as "not allowed". */
  refused?: boolean;
}

export interface PlannedEdit {
  id: string;
  attr: string;
  /** The value as written in the copied text; the grid turns it into a number or a boolean. */
  text: string;
}

/**
 * What a paste of `matrix` changes: one edit per cell that lands on an editable, typed attribute of
 * a listed row, with a value its enum allows; a cell the server refuses the person is counted as
 * refused, every other cell as skipped, never guessed at. A cell whose text equals what it holds is no change. Past `MAX_PASTE_CELLS` nothing is planned.
 */
export function planPaste(
  matrix: string[][],
  target: (rowOffset: number, colOffset: number) => PasteTarget,
): { edits: PlannedEdit[]; skipped: number; refused: number; tooLarge: boolean } {
  const cells = matrix.reduce((sum, row) => sum + row.length, 0);
  if (cells > MAX_PASTE_CELLS) {
    return { edits: [], skipped: cells, refused: 0, tooLarge: true };
  }
  const edits: PlannedEdit[] = [];
  let skipped = 0;
  let refused = 0;
  matrix.forEach((row, r) => {
    row.forEach((text, c) => {
      const cell = target(r, c);
      if (cell.refused) {
        refused++;
        return;
      }
      if (!cell.id || !cell.attr || cell.current === null || (cell.allowed && !cell.allowed.includes(text))) {
        skipped++;
        return;
      }
      if (text !== cell.current) {
        edits.push({ id: cell.id, attr: cell.attr, text });
      }
    });
  });
  return { edits, skipped, refused, tooLarge: false };
}
