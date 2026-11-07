import { describe, expect, it } from "vitest";
import { KEYS } from "./indicators";
import { LOCALES, stringsFor } from "./locales";

describe("locales", () => {
  it("says every string and every indicator in Slovak and English, and falls back to Slovak", () => {
    const keys = (strings: object) => Object.keys(strings).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.sk));
    for (const locale of [LOCALES.sk, LOCALES.en]) {
      for (const table of [locale.label, locale.question, locale.unit]) expect(Object.keys(table).sort()).toEqual([...KEYS].sort());
    }
    expect(stringsFor("en-GB").locale).toBe("en");
    expect(stringsFor("fi").locale).toBe("sk");
  });

  it("names a quarter the Slovak way", () => {
    expect(LOCALES.sk.quarter("Q2 2026")).toBe("2. štvrťrok 2026");
  });
});
