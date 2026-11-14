/**
 * What a typed cell is written as when the model names no rule: the stored value's own type
 * decides, so an edit never changes what kind of value an attribute holds.
 */
import { describe, expect, it } from "vitest";
import { coerce } from "../src/grid/GridCellEditors";

describe("a typed cell without a rule", () => {
  it("keeps a number a number and never writes a cleared cell as 0", () => {
    expect(coerce("4", 5)).toBe(4);
    expect(coerce("", 5)).toBe("");
    expect(coerce("  ", 5)).toBe("  ");
  });

  it("keeps a text a text however it reads", () => {
    expect(coerce("04002", "04001")).toBe("04002");
    expect(coerce("true", "yes")).toBe("true");
  });

  it("reads an empty cell's typed number, boolean and text as they read", () => {
    expect(coerce("true", undefined)).toBe(true);
    expect(coerce("Kamppi", undefined)).toBe("Kamppi");
  });

  it("follows the model's rule where there is one", () => {
    expect(coerce("7", "x", { kind: "integer" })).toBe(7);
    expect(coerce("7", 3, { kind: "string" })).toBe("7");
  });
});
