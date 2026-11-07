import { describe, expect, it } from "vitest";
import { LOCALES, stringsFor } from "./locales";

describe("locales", () => {
  it("says every string in Czech and English, and falls back to Czech", () => {
    const keys = (strings: object) => Object.keys(strings).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.cs));
    expect(stringsFor("en-GB").locale).toBe("en");
    expect(stringsFor("fi").locale).toBe("cs");
    expect(stringsFor(undefined).locale).toBe("cs");
  });

  it("says every sentence with its slots in both languages", () => {
    for (const strings of [LOCALES.cs, LOCALES.en]) {
      expect(strings.results(1)).toMatch(/^1 /);
      expect(strings.results(7)).toMatch(/^7 /);
      expect(strings.refused("Places", "no grant")).toContain("no grant");
      expect(strings.truncated("Places", 4000)).toContain("4000");
      expect(strings.inView("Places", 12)).toContain("12");
      expect(strings.inViewTruncated("Places", 4000)).toContain("4000");
    }
  });

  it("counts places the Czech way", () => {
    expect([1, 3, 5].map(LOCALES.cs.results)).toEqual(["1 místo", "3 místa", "5 míst"]);
  });
});
