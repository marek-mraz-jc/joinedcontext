/** T-6101: a unit measures a number, and a field that stops being a number loses its unit. */
import { describe, expect, it } from "vitest";
import { dropsUnit, rangeForUnit, unitNotice } from "../src/pages/models/unitRule";

describe("the unit and the type of value", () => {
  it("makes a text field a Number, a count an Integer, and leaves a number as it is", () => {
    expect(rangeForUnit("string", "CEL")).toBe("float");
    expect(rangeForUnit(undefined, "CEL")).toBe("float");
    expect(rangeForUnit("string", "C62")).toBe("integer");
    expect(rangeForUnit("integer", "CEL")).toBeUndefined();
    expect(rangeForUnit("double", "C62")).toBeUndefined();
    expect(rangeForUnit("string", "")).toBeUndefined();
    expect(unitNotice("string", "CEL")).toBe("madeNumber");
    expect(unitNotice("string", "C62")).toBe("madeInteger");
    expect(unitNotice("float", "CEL")).toBeUndefined();
  });

  it("drops the unit only of a field that leaves the numbers", () => {
    expect(dropsUnit("string", "CEL")).toBe(true);
    expect(dropsUnit("datetime", "CEL")).toBe(true);
    expect(dropsUnit("decimal", "CEL")).toBe(false);
    expect(dropsUnit("string", undefined)).toBe(false);
  });
});
