import { describe, expect, it } from "vitest";
import { COLUMNS, DATASETS, ENUMS, exportUrl, gridConfig } from "./datasets";
import { LOCALES, stringsFor } from "./locales";
import { answer, keyOf } from "./fixtures/registre";

const SLUG = "tw3jx6pcam2qzk7nre5bdv4yfh";

describe("the grids", () => {
  it("give each register its columns with the region's labels, the name pinned and every column filterable", () => {
    for (const dataset of DATASETS) {
      const grid = gridConfig(SLUG, dataset, LOCALES.sk.column);
      expect(grid.columns.map((c) => c.attr)).toEqual(COLUMNS[dataset]);
      expect(grid.columns.every((c) => LOCALES.sk.column[c.attr] === c.label)).toBe(true);
      expect(grid.columns.filter((c) => c.pinned).map((c) => c.attr)).toEqual(["name"]);
      expect(grid.filters.allowed).toEqual(COLUMNS[dataset]);
    }
  });

  it("label a column it has no word for by its attribute, and refuse a slug that is a path", () => {
    expect(gridConfig(SLUG, "areas", {}).columns.map((c) => c.label)).toEqual(COLUMNS.areas);
    expect(() => gridConfig("a/b", "areas", LOCALES.sk.column)).toThrow(/^grid areas: /);
  });

  it("are read from fixtures that answer nothing for a type the space does not hold", () => {
    expect(answer("Unknown")).toEqual([]);
    expect(answer(null)).toEqual([]);
    // T-4329: hospitals and social services are one PointOfInterest, told apart by `category`.
    const read = (query: string) => answer(keyOf(new URL(`http://portal.test/e?${query}`)));
    expect(read('type=PointOfInterest&q=category=="hospital"').every((r) => (r as { category: { value: string[] } }).category.value[0] === "hospital")).toBe(true);
    expect(read("type=PointOfInterest")).toHaveLength(4);
  });

  it("narrow the hospitals and the social services to their category, and no other register", () => {
    expect(gridConfig(SLUG, "hospitals", {}).filters.preset).toEqual({ q: 'category=="hospital"' });
    expect(gridConfig(SLUG, "social", {}).filters.preset).toEqual({ q: 'category=="socialService"' });
    expect(gridConfig(SLUG, "bridges", {}).filters.preset).toBeUndefined();
  });

  it("downloads a whole register from the endpoint's own files", () => {
    expect(exportUrl(SLUG, "bridges")).toBe(`/api/endpoint/${SLUG}/file.csv?type=Bridge&humanHeaders=true`);
  });
});

describe("locales", () => {
  it("say everything in Slovak and English, label every column and name every enum value", () => {
    const keys = (o: object) => Object.keys(o).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.sk));
    expect(keys(LOCALES.en.column)).toEqual(keys(LOCALES.sk.column));
    for (const attr of Object.values(COLUMNS).flat()) expect(LOCALES.sk.column[attr]).toBeTruthy();
    for (const value of Object.values(ENUMS).flat()) {
      expect(LOCALES.sk.values[value]).toBeTruthy();
      expect(LOCALES.en.values[value]).toBeTruthy();
    }
    expect(stringsFor("en").locale).toBe("en");
    expect(stringsFor("EN-gb").locale).toBe("en");
    // No language, or one it does not speak: Slovak, the region's own.
    expect(stringsFor(undefined).locale).toBe("sk");
    expect(stringsFor("de").locale).toBe("sk");
  });
});
