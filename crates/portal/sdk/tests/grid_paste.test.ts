import { describe, expect, it } from "vitest";
import { MAX_PASTE_CELLS, parseClipboard, planPaste } from "../src/grid/paste";
import type { PasteTarget } from "../src/grid/paste";

describe("pasting a range (T-3097)", () => {
  it("reads a spreadsheet's copy: tabs between cells, line breaks between rows, no empty last row", () => {
    expect(parseClipboard("1\t2\r\n3\t4\r\n")).toEqual([["1", "2"], ["3", "4"]]);
    expect(parseClipboard("only")).toEqual([["only"]]);
    expect(parseClipboard("a\n\nb")).toEqual([["a"], [""], ["b"]]);
  });

  it("sets what lands on editable typed cells and counts everything else as skipped", () => {
    const grid: Record<string, PasteTarget> = {
      "0:0": { id: "e1", attr: "bikes", current: "5" },
      "0:1": { id: "e1", attr: "status", current: "open", allowed: ["open", "closed"] },
      "1:0": { id: "e2", attr: "bikes", current: "3" },
      "1:1": { id: "e2", attr: undefined, current: null }, // not editable
    };
    const target = (r: number, c: number): PasteTarget => grid[`${r}:${c}`] ?? { id: undefined, attr: undefined, current: null };
    const plan = planPaste(parseClipboard("7\tclosed\n3\tx\n9\t9"), target);
    expect(plan.edits).toEqual([
      { id: "e1", attr: "bikes", text: "7" },
      { id: "e1", attr: "status", text: "closed" },
    ]);
    // e2.bikes unchanged (3), e2's second column not editable, the third row past the page.
    expect(plan.skipped).toBe(3);
    expect(plan.tooLarge).toBe(false);

    const refused = planPaste([["maybe"]], () => ({ id: "e1", attr: "status", current: "open", allowed: ["open", "closed"] }));
    expect(refused).toEqual({ edits: [], skipped: 1, refused: 0, tooLarge: false });
  });

  it("counts a cell the server refuses the person apart from the skipped ones (T-5530)", () => {
    const plan = planPaste([["1", "2", "3"]], (_r, c) =>
      c === 0 ? { id: "e1", attr: "a", current: "0" } : c === 1 ? { id: "e1", attr: undefined, current: null, refused: true } : { id: undefined, attr: undefined, current: null },
    );
    expect(plan).toEqual({ edits: [{ id: "e1", attr: "a", text: "1" }], skipped: 1, refused: 1, tooLarge: false });
  });

  it("plans nothing for a range past the bound", () => {
    // 500 cells, each a write the gateway checks against the model (T-4826, DM-86).
    expect(MAX_PASTE_CELLS).toBe(500);
    const wide = [Array.from({ length: MAX_PASTE_CELLS + 1 }, () => "1")];
    expect(planPaste(wide, () => ({ id: "e1", attr: "a", current: "0" }))).toEqual({
      edits: [],
      skipped: MAX_PASTE_CELLS + 1,
      refused: 0,
      tooLarge: true,
    });
  });
});
