import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Row } from "../src/ngsi";
import { filterRows, useFilters } from "../src/sdk/filters";
import type { FilterDef } from "../src/sdk/filters";

const ROWS: Row[] = [
  { id: "1", type: "Station", name: "Kaivopuisto", bikes: 12, seen: "2026-09-12T10:00:00Z" },
  { id: "2", type: "Station", name: "Kamppi", bikes: 3, seen: "2026-09-12T11:00:00Z" },
  { id: "3", type: "Station", name: "Pasila", bikes: null, seen: "2026-09-12T12:00:00Z" },
  { id: "4", type: "Station", name: "Hakaniemi", bikes: 0, seen: null },
];

const FILTERS: FilterDef[] = [
  { kind: "search", attrs: ["name"] },
  { kind: "select", attr: "name" },
  { kind: "range", attr: "bikes" },
  { kind: "dateRange", attr: "seen" },
];

describe("filterRows", () => {
  it("passes all when filters are blank or undefined", () => {
    expect(filterRows(ROWS, FILTERS, [undefined, "", undefined, undefined])).toHaveLength(4);
  });

  it("search filters case-insensitively across attributes", () => {
    const res = filterRows(ROWS, FILTERS, ["kam", undefined, undefined, undefined]);
    expect(res.map((r) => r.id)).toEqual(["2"]);
  });

  it("select matches exact formatted value", () => {
    const res = filterRows(ROWS, FILTERS, [undefined, "Pasila", undefined, undefined]);
    expect(res.map((r) => r.id)).toEqual(["3"]);
  });

  it("range drops rows without a number and keeps values within [min, max]", () => {
    const res = filterRows(ROWS, FILTERS, [undefined, undefined, [2, 10], undefined]);
    expect(res.map((r) => r.id)).toEqual(["2"]);
  });

  it("dateRange compares ISO strings with open lower or upper bound", () => {
    // Upper bound only
    const r1 = filterRows(ROWS, FILTERS, [undefined, undefined, undefined, ["", "2026-09-12T10:30:00Z"]]);
    expect(r1.map((r) => r.id)).toEqual(["1"]);

    // Lower bound only
    const r2 = filterRows(ROWS, FILTERS, [undefined, undefined, undefined, ["2026-09-12T11:30:00Z", ""]]);
    expect(r2.map((r) => r.id)).toEqual(["3"]);
  });
});

describe("useFilters hook", () => {
  it("provides shown rows, bind helper, and throws when bind index is out of range", () => {
    const { result } = renderHook(() => useFilters(ROWS, FILTERS));

    expect(result.current.shown).toHaveLength(4);

    act(() => {
      result.current.set(0, "kam");
    });
    expect(result.current.shown).toHaveLength(1);

    act(() => {
      result.current.reset();
    });
    expect(result.current.shown).toHaveLength(4);

    expect(() => result.current.bind(-1)).toThrow("out of range");
    expect(() => result.current.bind(10)).toThrow("out of range");

    const binding = result.current.bind(0);
    expect(binding.def).toBe(FILTERS[0]);
  });
});
