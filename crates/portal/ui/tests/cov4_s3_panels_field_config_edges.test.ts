// T-7706: fieldConfig edges: an escape inside a character class, the pattern cache filling up, no value to print, an object cell.
import { describe, expect, it } from "vitest";
import { displayValue, formatUnit, safeRegExp } from "../src/components/panels/fieldConfig";

describe("safeRegExp", () => {
  it("reads an escaped bracket inside a character class and still refuses a nested repeat after it", () => {
    expect(safeRegExp("[\\]a]+x")?.test("]x")).toBe(true);
    expect(safeRegExp("([\\]a]+)+")).toBeNull();
  });

  it("keeps working after more patterns than the cache holds", () => {
    for (let i = 0; i < 140; i++) expect(safeRegExp(`station-${i}`)?.test(`station-${i}`)).toBe(true);
    expect(safeRegExp("station-3")?.test("station-3")).toBe(true);
  });
});

describe("what a cell shows", () => {
  it("prints nothing for no number and an ellipsis for an object", () => {
    expect(formatUnit(null, "percent")).toBe("");
    expect(formatUnit(Number.NaN, undefined)).toBe("");
    expect(displayValue({ mappings: [], custom: {} }, { a: 1 })).toEqual({ text: "…" });
  });
});
