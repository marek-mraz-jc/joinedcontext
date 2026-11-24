// T-7706: formula.ts edges: an unclosed call, the range of a unary or min/max formula, and the refusals of the evaluator.
import { describe, expect, it } from "vitest";
import { evaluate, parseFormula, rangeOf } from "../src/components/entities/formula";

describe("parseFormula refusals", () => {
  it("names the missing closing bracket and where it was expected", () => {
    expect(() => parseFormula("max({a}, 1")).toThrow(/expected/);
    expect(() => parseFormula("({a} + 1")).toThrow(/`\)` expected/);
  });
});

describe("rangeOf", () => {
  it("is yes or no for not, a number for a minus, and follows the first argument of min and max", () => {
    expect(rangeOf(parseFormula("not {a}"))).toBe("boolean");
    expect(rangeOf(parseFormula("-{a}"))).toBe("float");
    expect(rangeOf(parseFormula("min('x', 'y')"))).toBe("string");
    expect(rangeOf(parseFormula("max({a}, 2)"))).toBe("float");
  });
});

describe("evaluate refusals and results", () => {
  const run = (text: string, values: Record<string, unknown> = {}) => evaluate(parseFormula(text), values);

  it("gives no value for not on a number, minus on a text, and and/or on non-booleans", () => {
    expect(run("not {a}", { a: 5 })).toBeNull();
    expect(run("-{a}", { a: "x" })).toBeNull();
    expect(run("{a} and {b}", { a: 1, b: true })).toBeNull();
    expect(run("{a} or {b}", { a: true, b: "x" })).toBeNull();
  });

  it("compares for difference and takes the absolute value", () => {
    expect(run("{a} != {b}", { a: 1, b: 2 })).toBe(true);
    expect(run("{a} != {b}", { a: 2, b: 2 })).toBe(false);
    expect(run("abs({a})", { a: -3 })).toBe(3);
  });

  it("gives no value for a function over a value that is no number", () => {
    expect(run("abs({a})", { a: "x" })).toBeNull();
    expect(run("abs({missing})")).toBeNull();
  });
});
