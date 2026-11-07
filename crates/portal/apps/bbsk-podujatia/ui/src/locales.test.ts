import { describe, expect, it } from "vitest";
import { LOCALES, stringsFor } from "./locales";

describe("locales", () => {
  it("says every string in Slovak and English, and falls back to Slovak", () => {
    const keys = (strings: object) => Object.keys(strings).sort();
    expect(keys(LOCALES.en)).toEqual(keys(LOCALES.sk));
    expect(Object.keys(LOCALES.en.view).sort()).toEqual(Object.keys(LOCALES.sk.view).sort());
    expect(LOCALES.en.weekdays).toHaveLength(7);
    expect(LOCALES.sk.weekdays).toHaveLength(7);
    expect(stringsFor("en-GB").locale).toBe("en");
    expect(stringsFor("fi").locale).toBe("sk");
    expect(stringsFor(undefined).locale).toBe("sk");
  });

  it("counts events the Slovak way and the English way", () => {
    expect([1, 3, 5].map(LOCALES.sk.results)).toEqual(["1 podujatie", "3 podujatia", "5 podujatí"]);
    expect([1, 2].map(LOCALES.en.results)).toEqual(["1 event", "2 events"]);
  });

  it("says a day, a range, a refusal and a cut-off list in both languages", () => {
    for (const strings of [LOCALES.sk, LOCALES.en]) {
      expect(strings.dayEvents("1. 11.", 0)).toContain("1. 11.");
      expect(strings.dayEvents("1. 11.", 2)).toContain("2");
      expect(strings.dateRangeHelp("2026-11-01", "2026-11-30")).toContain("2026-11-30");
      expect(strings.dateRangeHelp("", "2026-11-30")).not.toContain("2026-11-01");
      expect(strings.dateRangeHelp("2026-11-01", "")).toContain("2026-11-01");
      expect(strings.refused("403")).toContain("403");
      expect(strings.truncated(5000)).toContain("5000");
      expect(strings.notOnMap(2)).toContain("2");
      expect(strings.more(4)).toContain("4");
    }
  });
});
