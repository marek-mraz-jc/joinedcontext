import { describe, expect, it } from "vitest";
import { COLUMNS, DATASETS, exportUrl, gridConfig } from "./datasets";
import { answer } from "./fixtures/verejne";
import { LOCALES, stringsFor } from "./locales";

const SLUG = "fq4xw2ztnbe7rcav3ms6kd5ypu";

describe("the grids", () => {
  it("give each dataset its columns with the city's labels, the name pinned and every column filterable", () => {
    for (const dataset of DATASETS) {
      const grid = gridConfig(SLUG, dataset, LOCALES.sk.column);
      expect(grid.columns.map((c) => c.attr)).toEqual(COLUMNS[dataset]);
      expect(grid.columns.every((c) => LOCALES.sk.column[c.attr] === c.label)).toBe(true);
      expect(grid.columns.filter((c) => c.pinned).map((c) => c.attr)).toEqual(["name"]);
      expect(grid.filters.allowed).toEqual(COLUMNS[dataset]);
      expect(grid.mode).toBe("view");
    }
  });

  it("downloads a whole dataset from the endpoint's own files", () => {
    expect(exportUrl(SLUG, "schools")).toBe(`/api/endpoint/${SLUG}/file.csv?type=Organization&humanHeaders=true`);
  });
});

describe("locales", () => {
  it("say everything in Slovak and English and label every column", () => {
    const keys = (o: object) => Object.keys(o).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.sk));
    expect(keys(LOCALES.en.column)).toEqual(keys(LOCALES.sk.column));
    for (const attr of Object.values(COLUMNS).flat()) expect(LOCALES.sk.column[attr]).toBeTruthy();
    expect(stringsFor("en-US").locale).toBe("en");
    expect(stringsFor("de").locale).toBe("sk");
  });

  it("answers nothing for a type the public space does not hold", () => {
    expect(answer("Parking")).toEqual([]);
    expect(answer(null)).toEqual([]);
  });
});
