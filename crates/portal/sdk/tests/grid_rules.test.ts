import { describe, expect, it } from "vitest";
import { problemOf } from "../src/grid/rules";
import { DEFAULT_LABELS } from "../src/grid/useEntityGrid";

const L = DEFAULT_LABELS;

describe("problemOf", () => {
  it("passes anything without a rule, and an empty optional value", () => {
    expect(problemOf(undefined, "x", L)).toBeNull();
    expect(problemOf({ kind: "integer" }, "", L)).toBeNull();
  });
  it("refuses an empty required value", () => {
    expect(problemOf({ required: true }, "  ", L)).toBe(L.required);
  });
  it("checks numbers, whole numbers and bounds", () => {
    expect(problemOf({ kind: "number" }, "abc", L)).toBe(L.mustBeNumber);
    expect(problemOf({ kind: "integer" }, "1.5", L)).toBe(L.mustBeInteger);
    expect(problemOf({ kind: "number", minimum: 0 }, "-1", L)).toBe(`${L.atLeast} 0`);
    expect(problemOf({ kind: "number", maximum: 100 }, 101, L)).toBe(`${L.atMost} 100`);
    expect(problemOf({ kind: "integer", minimum: 0, maximum: 100 }, "100", L)).toBeNull();
  });
  it("checks booleans, dates and web addresses", () => {
    expect(problemOf({ kind: "boolean" }, "yes", L)).toBe(L.mustBeBoolean);
    expect(problemOf({ kind: "boolean" }, true, L)).toBeNull();
    expect(problemOf({ kind: "date" }, "not a date", L)).toBe(L.mustBeDate);
    expect(problemOf({ kind: "datetime" }, "2026-10-06T12:00:00Z", L)).toBeNull();
    expect(problemOf({ kind: "uri" }, "no scheme", L)).toBe(L.mustBeUri);
    expect(problemOf({ kind: "uri" }, "https://example.org/a", L)).toBeNull();
  });
  it("checks a pattern, and skips one the browser cannot compile", () => {
    expect(problemOf({ pattern: "^[A-Z]{2}$" }, "abc", L)).toBe(L.patternMismatch);
    expect(problemOf({ pattern: "^[A-Z]{2}$" }, "SK", L)).toBeNull();
    expect(problemOf({ pattern: "(" }, "anything", L)).toBeNull();
  });
});
