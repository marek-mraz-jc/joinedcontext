import { describe, expect, it } from "vitest";
import { COLUMNS, DATASETS, ENUMS, exportUrl, gridConfig } from "./datasets";
import { LOCALES, stringsFor } from "./locales";

const SLUG = "zd6qa2wmx7kc3nbr5tyhj4pve2";

describe("the grids", () => {
  it("give each dataset its columns with the city's labels, the first pinned and every column filterable", () => {
    for (const dataset of DATASETS) {
      const grid = gridConfig(SLUG, dataset, LOCALES.cs.column);
      expect(grid.columns.map((c) => c.attr)).toEqual(COLUMNS[dataset]);
      expect(grid.columns.every((c) => LOCALES.cs.column[c.attr] === c.label)).toBe(true);
      expect(grid.columns.filter((c) => c.pinned).map((c) => c.attr)).toEqual([COLUMNS[dataset][0]]);
      expect(grid.filters.allowed).toEqual(COLUMNS[dataset]);
    }
  });

  it("downloads a whole dataset from the endpoint's own files", () => {
    expect(exportUrl(SLUG, "budget")).toBe(`/api/endpoint/${SLUG}/file.csv?type=BudgetLine&humanHeaders=true`);
  });
});

describe("locales", () => {
  it("say everything in Czech and English, label every column and name every enum value", () => {
    const keys = (o: object) => Object.keys(o).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.cs));
    expect(keys(LOCALES.en.column)).toEqual(keys(LOCALES.cs.column));
    for (const attr of Object.values(COLUMNS).flat()) expect(LOCALES.cs.column[attr]).toBeTruthy();
    for (const value of Object.values(ENUMS).flat()) {
      expect(LOCALES.cs.values[value]).toBeTruthy();
      expect(LOCALES.en.values[value]).toBeTruthy();
    }
    expect(stringsFor("en").locale).toBe("en");
  });

  it("speaks English where served in English, and Czech for a language it does not have or none", () => {
    expect(stringsFor("en-GB")).toBe(LOCALES.en);
    expect(stringsFor("de")).toBe(LOCALES.cs);
    expect(stringsFor(undefined)).toBe(LOCALES.cs);
  });

  it("refuses a grid it could not build, naming the dataset and the finding", () => {
    expect(() => gridConfig("", "places", LOCALES.cs.column)).toThrow("grid places: must be a non-empty string");
  });
});
