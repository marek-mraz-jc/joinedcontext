/** The two locales say the same things, and the screen knows whose records it is showing. */
import { describe, expect, it } from "vitest";
import { bodyOf, LOCALES, noteWords, SPACE_OF, stringsFor } from "./locales";
import { COLUMNS, NOTE_MAX } from "./records";

describe("the locales", () => {
  it("falls back to Slovak, which is the language of both publishers", () => {
    expect(stringsFor(undefined)).toBe(LOCALES.sk);
    expect(stringsFor("de")).toBe(LOCALES.sk);
    expect(stringsFor("sk-SK")).toBe(LOCALES.sk);
    expect(stringsFor("en-GB")).toBe(LOCALES.en);
  });

  it("has the same keys in both, so a Slovak page never falls through to English", () => {
    const keys = (value: unknown, path = ""): string[] =>
      typeof value === "object" && value !== null
        ? Object.entries(value as Record<string, unknown>).flatMap(([k, v]) => keys(v, `${path}.${k}`))
        : [path];
    expect(keys(LOCALES.sk).sort()).toEqual(keys(LOCALES.en).sort());
  });

  it("names every column the grid shows, in both", () => {
    for (const locale of Object.values(LOCALES)) {
      for (const attr of COLUMNS) {
        expect(locale.column[attr]?.trim(), attr).not.toBe("");
      }
    }
  });

  it("says on the page why the figures cannot be edited", () => {
    // The sentence is the application's, not a comment in its source: a person who sees a
    // read-only column has to be told who owns the number.
    expect(LOCALES.sk.readOnlyWhy).toContain("kanál");
    expect(LOCALES.en.readOnlyWhy).toContain("pipeline");
  });

  it("fills the model's own bound into the refusal a person reads", () => {
    expect(noteWords(LOCALES.en).tooLong(NOTE_MAX)).toContain(String(NOTE_MAX));
    expect(noteWords(LOCALES.sk).tooLong(NOTE_MAX)).toContain(String(NOTE_MAX));
    expect(noteWords(LOCALES.sk).tooLong(NOTE_MAX)).not.toContain("{max}");
  });
});

describe("whose records these are", () => {
  it("is decided by the space the configuration names, never by a build flag", () => {
    expect(bodyOf(SPACE_OF.banskabystrica)).toBe("banskabystrica");
    expect(bodyOf(SPACE_OF.bbsk)).toBe("bbsk");
  });

  it("is nobody's when the space is one this application does not know", () => {
    expect(bodyOf("helsinki")).toBeNull();
    expect(bodyOf(undefined)).toBeNull();
  });

  it("names each body on its own heading, and neither under the other's", () => {
    for (const locale of Object.values(LOCALES)) {
      expect(locale.title.banskabystrica).not.toBe(locale.title.bbsk);
      expect(locale.source.banskabystrica).toContain("banskabystrica-mesto");
      expect(locale.source.bbsk).toContain("bbsk-kraj");
    }
  });
});
