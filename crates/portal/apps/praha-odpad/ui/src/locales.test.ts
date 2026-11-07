import { describe, expect, it } from "vitest";
import { LOCALES, stringsFor } from "./locales";

describe("locales", () => {
  it("say everything in Czech and English", () => {
    const keys = (o: object) => Object.keys(o).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.cs));
    expect(keys(LOCALES.en.values)).toEqual(keys(LOCALES.cs.values));
    expect(LOCALES.en.csvHeader).toHaveLength(LOCALES.cs.csvHeader.length);
    expect(stringsFor("en").locale).toBe("en");
    expect(stringsFor("sk").locale).toBe("cs");
  });

  it("say how long ago a reading was", () => {
    expect([0.5, 5, 72].map(LOCALES.cs.ago)).toEqual(["před méně než hodinou", "před 5 h", "před 3 dny"]);
  });

  it("say every sentence with its slots, in both languages", () => {
    for (const strings of [LOCALES.cs, LOCALES.en]) {
      for (const [key, value] of Object.entries(strings)) {
        if (typeof value === "function") expect((value as (...a: unknown[]) => string)(3, "x"), key).toMatch(/\S/);
      }
      expect([0.5, 5, 30, 72].map(strings.ago).every((said) => said.length > 0)).toBe(true);
    }
    expect(stringsFor(undefined).locale).toBe("cs");
  });
});
