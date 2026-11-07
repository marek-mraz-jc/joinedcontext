/**
 * Computed columns (T-6384): a formula over a row's attributes is parsed and walked, never run as
 * code, and the grid shows its value as a column it neither edits nor asks the endpoint about.
 */
import { describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { evaluate, parseFormula, withFormulas } from "../src/grid/formula";
import { fixtureSource } from "../src/grid/source";
import type { FilterColumn } from "../src/grid/filters";
import type { RichRow } from "../src/grid/model";

const value = (text: string, row: Record<string, unknown>) => {
  const parsed = parseFormula(text);
  if (!parsed.ok) throw new Error(parsed.error);
  return evaluate(parsed.formula, (name) => row[name]);
};

describe("parseFormula and evaluate", () => {
  it("works out arithmetic and comparisons in the usual precedence", () => {
    const station = { available: 3, total: 12, free: 9 };
    expect(value("available / total", station)).toBe(0.25);
    expect(value("available + free * 2", station)).toBe(21);
    expect(value("(available + free) * 2", station)).toBe(24);
    expect(value("-available % 2", station)).toBe(-1);
    expect(value("available == 0", station)).toBe(false);
    expect(value("available < 5 && free >= 9", station)).toBe(true);
    expect(value("!(available > 5) || false", station)).toBe(true);
    expect(value("available == 0 ? 100 : available / total * 100", station)).toBe(25);
  });

  it("names the attributes a formula reads, once each", () => {
    const parsed = parseFormula("a / (a + b)");
    expect(parsed.ok && parsed.attrs).toEqual(["a", "b"]);
  });

  it("leaves a cell empty for a missing value, text or a division by zero", () => {
    expect(value("available / total", { available: 3 })).toBeNull();
    expect(value("available / total", { available: 3, total: "12" })).toBeNull();
    expect(value("available / total", { available: 3, total: 0 })).toBeNull();
    expect(value("available % total", { available: 3, total: 0 })).toBeNull();
    expect(value("available > 1 ? 1 : 0", {})).toBeNull();
    expect(value("available == true", { available: 1 })).toBeNull();
  });

  it("refuses code: calls, members, strings, assignment and templates", () => {
    for (const code of [
      "alert(1)",
      "constructor.constructor",
      "this['x']",
      '"a" + b',
      "a = 1",
      "a; b",
      "`${a}`",
      "a => a",
      "a[0]",
      "{}",
    ]) {
      const parsed = parseFormula(code);
      expect(parsed.ok, code).toBe(false);
    }
    const call = parseFormula("total + fetch(1)");
    expect(call).toEqual({ ok: false, error: '"fetch(" is a function, which a formula cannot use: write attribute names, numbers and + - * / only', at: 8 });
  });

  it("refuses an attribute the type does not have when the names are known", () => {
    expect(parseFormula("available / totl", ["available", "total"])).toEqual({ ok: false, error: '"totl" is no attribute of this type', at: 12 });
    expect(parseFormula("available / total", ["available", "total"]).ok).toBe(true);
  });

  it("refuses an unfinished formula, too long a text and too deep a nesting", () => {
    expect(parseFormula("").ok).toBe(false);
    expect(parseFormula("a +").ok).toBe(false);
    expect(parseFormula("(a + b").ok).toBe(false);
    expect(parseFormula("a ? b").ok).toBe(false);
    expect(parseFormula("a b").ok).toBe(false);
    expect(parseFormula("a + ".repeat(200) + "a").ok).toBe(false);
    expect(parseFormula("(".repeat(40) + "a" + ")".repeat(40)).ok).toBe(false);
  });
});

describe("computed columns in the grid configuration", () => {
  const base = { source: { kind: "fixture", name: "t" }, type: "Station" };

  it("keeps a formula that parses", () => {
    const { config, findings } = parseGridConfig({ ...base, columns: [{ attr: "occupancy", formula: "bikes / slots" }] });
    expect(findings).toEqual([]);
    expect(config?.columns[0].formula).toBe("bikes / slots");
  });

  it("names the field and the position of a formula it refuses", () => {
    const { findings } = parseGridConfig({ ...base, columns: [{ attr: "x", formula: "bikes + eval(1)" }] });
    expect(findings).toEqual([{ path: "/columns/0/formula", message: '"eval(" is a function, which a formula cannot use: write attribute names, numbers and + - * / only (at character 9)' }]);
  });

  it("refuses a computed column that reads itself or is edited", () => {
    expect(parseGridConfig({ ...base, columns: [{ attr: "bikes", formula: "bikes * 2" }] }).findings[0].path).toBe("/columns/0/formula");
    expect(parseGridConfig({ ...base, columns: [{ attr: "x", formula: "bikes", editable: true }] }).findings[0].path).toBe("/columns/0/editable");
    const listed = parseGridConfig({ ...base, mode: "edit", editableAttrs: ["x"], columns: [{ attr: "x", formula: "bikes" }] });
    expect(listed.findings).toEqual([{ path: "/editableAttrs/0", message: '"x" is a computed column and cannot be edited' }]);
  });
});

const stations = [
  { id: "urn:ngsi-ld:Station:1", type: "Station", name: { type: "Property", value: "Kamppi" }, bikes: { type: "Property", value: 3 }, slots: { type: "Property", value: 12 } },
  { id: "urn:ngsi-ld:Station:2", type: "Station", name: { type: "Property", value: "Kallio" }, bikes: { type: "Property", value: 0 }, slots: { type: "Property", value: 8 } },
  { id: "urn:ngsi-ld:Station:3", type: "Station", name: { type: "Property", value: "Pasila" }, bikes: { type: "Property", value: 4 } },
];

describe("withFormulas", () => {
  it("adds the value as a cell and leaves the stored attributes alone", () => {
    const rows: RichRow[] = [{ id: "a", type: "Station", cells: { bikes: { kind: "property", value: 3 }, slots: { kind: "property", value: 12 } }, raw: {} }];
    const parsed = parseFormula("bikes / slots");
    if (!parsed.ok) throw new Error(parsed.error);
    const [row] = withFormulas(rows, [{ attr: "share", formula: parsed.formula, reads: parsed.attrs }]);
    expect(row.cells.share).toEqual({ kind: "property", value: 0.25 });
    expect(row.cells.bikes).toEqual({ kind: "property", value: 3 });
    expect(rows[0].cells.share).toBeUndefined();
  });
});

describe("a computed column in the EntityGrid", () => {
  it("shows each row's value, an empty cell where an input is missing, and offers no endpoint filter on it", async () => {
    const { config } = parseGridConfig({
      source: { kind: "fixture", name: "t" },
      type: "Station",
      columns: [{ attr: "name", label: "Name", pinned: true }, { attr: "bikes" }, { attr: "empty", label: "Empty", formula: "bikes == 0" }, { attr: "share", label: "Share", formula: "bikes / slots * 100" }],
      pageSize: 10,
    });
    const seen: FilterColumn[][] = [];
    render(<EntityGrid config={config!} source={fixtureSource(stations)} onFilterColumns={(c) => seen.push(c)} />);
    await waitFor(() => expect(screen.getByText("Kamppi")).toBeTruthy());
    const rowOf = (name: string) => screen.getByText(name).closest("[role=row]") as HTMLElement;
    expect(rowOf("Kamppi").textContent).toContain("25");
    expect(rowOf("Kallio").textContent).toMatch(/0\D*(Yes|true)|(Yes|true)/);
    const keys = seen.at(-1)!.map((c) => c.attr);
    expect(keys).toContain("bikes");
    expect(keys).not.toContain("share");
    expect(keys).not.toContain("empty");
  });
});
