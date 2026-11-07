import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// T-4242 (T-4162 #6): at a phone's width the pinned name keeps room to read, and the table shows
// that more columns wait to its right instead of cutting them off with no hint.
describe("the grid at a phone's width", () => {
  const css = readFileSync("src/grid/grid.css", "utf8");

  // T-6210: the pinned column is the id, so on a phone it keeps a quarter of the screen and leaves
  // the cells to its right reachable instead of covering them.
  it("keeps the pinned id a column to read in on a narrow screen", () => {
    const narrow = css.slice(css.indexOf("@media (width <= 640px)"));
    expect(narrow).toMatch(/\.jc-grid td\.jc-grid-pinned[^{]*\{[^}]*min-width:\s*min\(25vw, 7rem\)/);
  });

  it("shadows the right edge while the table scrolls on", () => {
    const scroll = css.match(/\.jc-grid-scroll\s*\{[^}]*\}/)?.[0] ?? "";
    expect(scroll).toMatch(/background:[^;]*\blocal\b[^;]*\bscroll\b/s);
  });
});
