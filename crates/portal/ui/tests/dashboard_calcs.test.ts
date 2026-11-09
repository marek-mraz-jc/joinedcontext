/**
 * Every Grafana reducer over every edge case, against what Grafana 12.4.14 itself computed
 * (tests/fixtures/grafana-reference/calcs.json, recorded by scripts/grafana-calcs.mjs from the
 * reference instance's Reduce transformation). The reducer list is the reference's own.
 */
import { describe, expect, it } from "vitest";
import calcs from "./fixtures/grafana-reference/calcs.json";
import { REDUCERS, reduceField, reducerId } from "../src/components/panels/reducers";
import type { CalcField, CalcValue } from "../src/components/panels/reducers";
import { reduce, reducedOf } from "../src/components/panels/frames";

type Case = keyof typeof calcs.cases;

/** A case as TestData's csv_content gives it: a number field, or a text field when one cell is not a number. */
function fieldOf(values: readonly unknown[]): CalcField {
  const numeric = values.every((value) => value === null || typeof value === "number" || value === "NaN" || value === "Inf");
  return {
    numeric,
    values: values.map((value): CalcValue => {
      if (value === null) return null;
      if (!numeric) return String(value);
      if (value === "NaN") return Number.NaN;
      if (value === "Inf") return Number.POSITIVE_INFINITY;
      return value as number;
    }),
  };
}

/** A result as the fixture writes it: NaN and null alike as null (the inspector cannot tell them apart). */
function written(value: unknown): unknown {
  if (value === undefined) return "undefined";
  if (typeof value === "number") {
    if (Number.isNaN(value)) return null;
    if (value === Number.POSITIVE_INFINITY) return "Infinity";
    if (value === Number.NEGATIVE_INFINITY) return "-Infinity";
  }
  return Array.isArray(value) ? value.map((one) => (typeof one === "number" && !Number.isFinite(one) ? null : one)) : value;
}

describe("Grafana's reducers", () => {
  it("are the reference's own list, under its own names", () => {
    expect(Object.keys(REDUCERS).sort()).toEqual([...calcs.reducers].sort());
    expect(calcs.reducers.map((id) => REDUCERS[id].name)).toEqual(calcs.names);
  });

  const cases = (Object.keys(calcs.cases) as Case[]).filter((name) => name !== "empty");
  it.each(cases)("compute what Grafana computes for %s", (name) => {
    const got = reduceField(fieldOf(calcs.cases[name]), calcs.reducers);
    const expected = calcs.values[name] as Record<string, unknown>;
    const mismatches = calcs.reducers
      .map((id, at) => ({ id, want: expected[calcs.names[at]], got: written(got[id]) }))
      .filter(({ want, got }) => JSON.stringify(want) !== JSON.stringify(got));
    expect(mismatches).toEqual([]);
  });

  it("give each reducer's empty result for an empty field (Grafana's table drops that row)", () => {
    expect(calcs.values.empty).toEqual(Object.fromEntries(calcs.names.map((name) => [name, "undefined"])));
    const got = reduceField({ values: [], numeric: true }, calcs.reducers);
    expect([got.sum, got.count, got.countAll, got.allIsZero, got.allIsNull, got.max, got.delta]).toEqual([0, 0, 0, false, true, null, null]);
  });

  it("give the same value alone as among all the others", () => {
    for (const name of cases) {
      const field = fieldOf(calcs.cases[name]);
      const all = reduceField(field, calcs.reducers);
      for (const id of calcs.reducers) expect(written(reduceField(field, [id])[id]), `${name} ${id}`).toEqual(written(all[id]));
    }
  });

  it("resolve Grafana's aliases", () => {
    expect(["current", "avg", "total", "max"].map(reducerId)).toEqual(["lastNotNull", "mean", "sum", "max"]);
    expect(reduce([1, 5, null], "current")).toBe(5);
  });

  it("count a counter reset the way Grafana's delta does", () => {
    expect(reduce([10, 20, 30, 5, 15, 25], "delta")).toBe(30);
    expect(reduce([10, 20, 30, 5], "delta")).toBe(20);
    expect(reduce([9, 7, 4], "delta")).toBe(0);
  });

  it("draw a boolean result as 1 or 0 and a NaN as no value", () => {
    expect(reduce([0, 0], "allIsZero")).toBe(1);
    expect(reduce([1, Number.NaN], "variance")).toBeNull();
  });

  it("meet a panel's NaN and Inf texts as Grafana's backend frames carry them", () => {
    const frame = { name: "", refId: "A", fields: [{ name: "Value", type: "number" as const, values: [1, "NaN", 5] }] };
    expect(reducedOf([frame], "mean")[0].value).toBe(3);
    expect(reducedOf([frame], "variance")[0].value).toBeNull();
    expect(reducedOf([{ ...frame, fields: [{ ...frame.fields[0], values: [1, "+Inf"] }] }], "max")[0].value).toBe(Number.POSITIVE_INFINITY);
  });
});
