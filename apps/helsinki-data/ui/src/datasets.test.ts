import { describe, expect, it } from "vitest";
import { COLUMNS, DATASETS, ENUMS, exportUrl, gridConfig } from "./datasets";
import { LOCALES, stringsFor } from "./locales";

const SLUG = "mc4hz7tkq2vbr6xna3wjd5yep2";

describe("the grids", () => {
  it("give each dataset its columns with the city's labels, the name pinned and every column filterable", () => {
    for (const dataset of DATASETS) {
      const grid = gridConfig(SLUG, dataset, LOCALES.fi.column);
      expect(grid.columns.map((c) => c.attr)).toEqual(COLUMNS[dataset]);
      expect(grid.columns.every((c) => LOCALES.fi.column[c.attr] === c.label)).toBe(true);
      expect(grid.columns.filter((c) => c.pinned).map((c) => c.attr)).toEqual([COLUMNS[dataset][0]]);
      expect(grid.filters.allowed).toEqual(COLUMNS[dataset]);
    }
  });

  it("downloads a whole dataset from the endpoint's own files", () => {
    expect(exportUrl(SLUG, "permits")).toBe(`/api/endpoint/${SLUG}/file.csv?type=CityWork&humanHeaders=true`);
    // T-4329: the zones are OnStreetParking beside the parking guide's areas; only the zones are read.
    expect(exportUrl(SLUG, "zones")).toBe(`/api/endpoint/${SLUG}/file.csv?type=OnStreetParking&q=category%3D%3D%22feeCharged%22%2C%22forResidents%22&humanHeaders=true`);
    expect(gridConfig(SLUG, "zones", {}).filters.preset).toEqual({ q: 'category=="feeCharged","forResidents"' });
    expect(gridConfig(SLUG, "permits", {}).filters.preset).toBeUndefined();
  });
});

describe("locales", () => {
  it("say everything in Finnish and English, label every column and name every enum value", () => {
    const keys = (o: object) => Object.keys(o).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.fi));
    expect(keys(LOCALES.en.column)).toEqual(keys(LOCALES.fi.column));
    for (const attr of Object.values(COLUMNS).flat()) expect(LOCALES.fi.column[attr]).toBeTruthy();
    for (const value of Object.values(ENUMS).flat()) {
      expect(LOCALES.fi.values[value]).toBeTruthy();
      expect(LOCALES.en.values[value]).toBeTruthy();
    }
    expect(stringsFor("en").locale).toBe("en");
  });

  it("speaks English where served in English, and Finnish for a language it does not have or none", () => {
    expect(stringsFor("en-GB")).toBe(LOCALES.en);
    expect(stringsFor("sv")).toBe(LOCALES.fi);
    expect(stringsFor(undefined)).toBe(LOCALES.fi);
  });

  it("refuses a grid it could not build, naming the dataset and the finding", () => {
    expect(() => gridConfig("", "services", LOCALES.fi.column)).toThrow("grid services: must be a non-empty string");
  });
});
