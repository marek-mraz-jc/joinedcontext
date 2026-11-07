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
  });

  it("put what they are given into every sentence they build, in both languages", () => {
    for (const strings of [LOCALES.sk, LOCALES.en]) {
      const built = Object.entries(strings).filter(([, v]) => typeof v === "function");
      expect(built.map(([k]) => k)).toContain("shared");
      for (const [key, sentence] of built) {
        expect((sentence as (arg: number | string) => string)(37), key).toContain("37");
      }
    }
  });
});
