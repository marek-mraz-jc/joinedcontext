/** The two locales say the same things, and neither leaves the other's words on the page. */
import { describe, expect, it } from "vitest";
import { BAND_SHAPE, LOCALES, nameOf, stringsFor } from "./locales";

describe("the locales", () => {
  it("falls back to Slovak, which is the language of the city that publishes this", () => {
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

  it("names every band, and gives each one a shape so the colour is never alone", () => {
    for (const locale of Object.values(LOCALES)) {
      for (const band of ["low", "raised", "high", "stale", "unknown"] as const) {
        expect(locale.band[band].trim(), band).not.toBe("");
        expect(BAND_SHAPE[band], band).not.toBe("");
      }
    }
  });

  it("says what the limit is and what an old reading means, on the page itself", () => {
    expect(LOCALES.sk.limitNote).toContain("50");
    expect(LOCALES.en.limitNote).toContain("50");
    expect(LOCALES.sk.staleNote).toContain("tri hodiny");
    expect(LOCALES.en.staleNote).toContain("three hours");
  });

  it("names a station by the name it publishes, and after its own id when it publishes none", () => {
    expect(nameOf({ localId: "eea-SK0263A", name: "Stanica SK0263A, mestské pozadie" }, LOCALES.sk)).toBe(
      "Stanica SK0263A, mestské pozadie",
    );
    expect(nameOf({ localId: "station-1", name: null }, LOCALES.sk)).toBe("Stanica 1");
    expect(nameOf({ localId: "station-1", name: null }, LOCALES.en)).toBe("Station 1");
    // An id in another shape is shown as it is rather than dressed up.
    expect(nameOf({ localId: "mesto-juh", name: null }, LOCALES.en)).toBe("Station mesto-juh");
  });
});
