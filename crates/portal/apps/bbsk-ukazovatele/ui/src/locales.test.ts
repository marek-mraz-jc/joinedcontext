/** The two locales say the same things, and neither leaves the other's words on the page. */
import { describe, expect, it } from "vitest";
import { LOCALES, stringsFor } from "./locales";
import { UNIT_CODE } from "./indicators";

const TERRITORIES = [
  "kraj", "mesto",
  "okres-banska-bystrica", "okres-banska-stiavnica", "okres-brezno", "okres-detva",
  "okres-krupina", "okres-lucenec", "okres-poltar", "okres-revuca", "okres-rimavska-sobota",
  "okres-velky-krtis", "okres-zvolen", "okres-zarnovica", "okres-ziar-nad-hronom",
];

describe("the locales", () => {
  it("falls back to Slovak, which is the language of both publishers", () => {
    expect(stringsFor(undefined)).toBe(LOCALES.sk);
    expect(stringsFor("de")).toBe(LOCALES.sk);
    expect(stringsFor("sk-SK")).toBe(LOCALES.sk);
    expect(stringsFor("en-GB")).toBe(LOCALES.en);
  });

  it("names every territory the contract declares, in both", () => {
    for (const locale of Object.values(LOCALES)) {
      expect(Object.keys(locale.territory).sort()).toEqual([...TERRITORIES].sort());
    }
  });

  it("names every indicator the contract declares, with a written unit, in both", () => {
    for (const locale of Object.values(LOCALES)) {
      expect(Object.keys(locale.indicator).sort()).toEqual(Object.keys(UNIT_CODE).sort());
      for (const [key, entry] of Object.entries(locale.indicator)) {
        expect(entry.title.trim(), key).not.toBe("");
        expect(entry.unit.trim(), key).not.toBe("");
      }
    }
  });

  it("has the same keys in both, so a Slovak page never falls through to English", () => {
    const keys = (value: unknown, path = ""): string[] =>
      typeof value === "object" && value !== null
        ? Object.entries(value as Record<string, unknown>).flatMap(([k, v]) => keys(v, `${path}.${k}`))
        : [path];
    expect(keys(LOCALES.sk).sort()).toEqual(keys(LOCALES.en).sort());
  });

  it("says the region is the bigger body, on the card's own note", () => {
    // The factor of eight is the misreading this whole screen is arranged against.
    expect(LOCALES.sk.bodyNote.banskabystrica).toContain("osemkrát");
    expect(LOCALES.en.bodyNote.banskabystrica).toContain("eight times");
  });
});
