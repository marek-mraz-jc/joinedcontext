import { describe, expect, it } from "vitest";
import { LOCALES, stringsFor } from "./locales";

describe("locales", () => {
  it("says every string in Slovak and English, and falls back to Slovak", () => {
    const keys = (strings: object) => Object.keys(strings).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.sk));
    expect(Object.keys(LOCALES.en.pollutant).sort()).toEqual(Object.keys(LOCALES.sk.pollutant).sort());
    expect(stringsFor("en-GB").locale).toBe("en");
    expect(stringsFor("fi").locale).toBe("sk");
    expect(stringsFor(undefined).locale).toBe("sk");
  });

  it("counts the Slovak way", () => {
    expect([1, 3, 5].map(LOCALES.sk.results)).toEqual(["1 miesto", "3 miesta", "5 miest"]);
    expect([1, 2, 42].map(LOCALES.sk.departuresToday)).toEqual(["1 odchod dnes", "2 odchody dnes", "42 odchodov dnes"]);
  });

  it("counts and says each failure in English", () => {
    const en = LOCALES.en;
    expect([1, 2].map(en.results)).toEqual(["1 place", "2 places"]);
    expect([1, 2].map(en.departuresToday)).toEqual(["1 train leaving today", "2 trains leaving today"]);
    expect(en.refused("Monuments", "no")).toBe("Monuments could not be loaded: no");
    expect(en.truncated("Monuments", 1000)).toBe("Monuments: the first 1000 are shown; narrow the search.");
    expect(LOCALES.sk.refused("Pamiatky", "nie")).toContain("nie");
    expect(LOCALES.sk.truncated("Pamiatky", 1000)).toContain("1000");
  });
});
