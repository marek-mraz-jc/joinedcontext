import { describe, expect, it } from "vitest";
import { LOCALES, stringsFor } from "./locales";

describe("locales", () => {
  it("say everything in Slovak and English", () => {
    const keys = (o: object) => Object.keys(o).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.sk));
    expect(keys(LOCALES.en.column)).toEqual(keys(LOCALES.sk.column));
    expect(LOCALES.en.csvHeader).toHaveLength(LOCALES.sk.csvHeader.length);
    expect(stringsFor("en").locale).toBe("en");
    expect(stringsFor("cs").locale).toBe("sk");
    expect(Object.keys(LOCALES.en.values).sort()).toEqual(Object.keys(LOCALES.sk.values).sort());
    expect([1, 3, 5].map(LOCALES.sk.years)).toEqual(["1 rok", "3 roky", "5 rokov"]);
    expect([1, 2].map(LOCALES.en.years)).toEqual(["1 year", "2 years"]);
  });

  it("say a failure, a column to sort by and a cut-off list in both languages", () => {
    for (const strings of [LOCALES.sk, LOCALES.en]) {
      expect(strings.failed("403")).toContain("403");
      expect(strings.sortBy(strings.column.length)).toContain(strings.column.length);
      expect(strings.truncated(5000)).toContain("5000");
    }
    expect(stringsFor(undefined).locale).toBe("sk");
  });
});
