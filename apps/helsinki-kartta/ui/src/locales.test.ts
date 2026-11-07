import { describe, expect, it } from "vitest";
import { LOCALES, stringsFor } from "./locales";

describe("locales", () => {
  it("says every string in Finnish and English, and falls back to Finnish", () => {
    const keys = (strings: object) => Object.keys(strings).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.fi));
    expect(Object.keys(LOCALES.en.kind).sort()).toEqual(Object.keys(LOCALES.fi.kind).sort());
    expect(stringsFor("en-GB").locale).toBe("en");
    expect(stringsFor("sv").locale).toBe("fi");
    expect(stringsFor(undefined).locale).toBe("fi");
  });

  it("counts places the Finnish way and the English way", () => {
    expect([1, 3].map(LOCALES.fi.results)).toEqual(["1 paikka", "3 paikkaa"]);
    expect([1, 3].map(LOCALES.en.results)).toEqual(["1 place", "3 places"]);
  });

  it("says why a layer is missing or cut short, in both languages", () => {
    expect(LOCALES.en.refused("Services", "forbidden")).toBe("Services could not be loaded: forbidden");
    expect(LOCALES.en.truncated("Services", 4000)).toBe("Services: the first 4000 are shown; narrow the search.");
    expect(LOCALES.fi.refused("Palvelut", "kielletty")).toBe("Palvelut: lataus epäonnistui: kielletty");
    expect(LOCALES.fi.truncated("Palvelut", 4000)).toBe("Palvelut: näytetään ensimmäiset 4000; tarkenna hakua.");
  });
});
