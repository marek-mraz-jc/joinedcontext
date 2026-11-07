import { describe, expect, it } from "vitest";
import { LOCALES, stringsFor } from "./locales";

describe("locales", () => {
  it("says every string in Slovak and English, and falls back to Slovak", () => {
    const keys = (strings: object) => Object.keys(strings).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.sk));
    expect(Object.keys(LOCALES.en.kind).sort()).toEqual(Object.keys(LOCALES.sk.kind).sort());
    expect(stringsFor("en-GB").locale).toBe("en");
    expect(stringsFor("fi").locale).toBe("sk");
    expect(stringsFor(undefined).locale).toBe("sk");
  });

  it("says a count, a refusal and a cut-off list in both languages", () => {
    expect([1, 2].map(LOCALES.en.results)).toEqual(["1 place", "2 places"]);
    for (const strings of [LOCALES.sk, LOCALES.en]) {
      expect(strings.refused(strings.kind.hospital, "403")).toContain("403");
      expect(strings.truncated(strings.kind.social, 1000)).toContain("1000");
    }
  });

  it("counts places the Slovak way", () => {
    expect([1, 3, 5].map(LOCALES.sk.results)).toEqual(["1 miesto", "3 miesta", "5 miest"]);
  });
});
