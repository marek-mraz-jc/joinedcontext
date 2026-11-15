import { describe, expect, it } from "vitest";
import { DAYS, formatHelsinkiTime, langOf, number, t } from "./i18n";
import type { Word } from "./i18n";

const ALL_WORDS: Word[] = [
  "title",
  "page",
  "station",
  "searchStation",
  "bikesNow",
  "freeSlots",
  "totalSlots",
  "mapTitle",
  "mapNotice",
  "chartTitle",
  "chartEmpty",
  "profileTitle",
  "profileTable",
  "hour",
  "day",
  "mon",
  "tue",
  "wed",
  "thu",
  "fri",
  "sat",
  "sun",
  "weatherEffect",
  "weatherAssumption",
  "noRain",
  "rain",
  "tooLittleHistory",
  "noWeatherTerms",
  "weatherUnreadable",
  "loading",
  "analysing",
  "emptyStations",
  "noHistory",
  "estimationFailed",
  "details",
  "bikesHistorySeries",
  "estimateSeries",
  "estimateBand",
  "nearestWeather",
];

describe("i18n translations", () => {
  it("every key exists in en and fi, no empty string", () => {
    for (const key of ALL_WORDS) {
      const en = t("en", key);
      const fi = t("fi", key);

      expect(typeof en).toBe("string");
      expect(typeof fi).toBe("string");
      expect(en.trim().length).toBeGreaterThan(0);
      expect(fi.trim().length).toBeGreaterThan(0);
    }
  });

  it("substitutes placeholders correctly", () => {
    expect(
      t("en", "weatherEffect", {
        perDegree: "+0.5",
        rain: "−2.0",
        hours: 48,
      }),
    ).toBe("1 °C warmer: +0.5 bikes; rain: −2.0 bikes; from 48 hours with weather");

    expect(
      t("fi", "weatherEffect", {
        perDegree: "+0.5",
        rain: "−2.0",
        hours: 48,
      }),
    ).toBe("1 °C lämpimämpi: +0.5 pyörää; sade: −2.0 pyörää; 48 tunnista sään kanssa");

    expect(
      t("en", "weatherAssumption", {
        time: "12:00",
        temp: "15",
        rainState: "no rain",
      }),
    ).toBe("The weather stays as it was at 12:00: 15 °C, no rain");

    expect(
      t("fi", "weatherAssumption", {
        time: "12:00",
        temp: "15",
        rainState: "ei sadetta",
      }),
    ).toBe("Sää pysyy samana kuin klo 12:00: 15 °C, ei sadetta");

    expect(t("en", "nearestWeather", { name: "Kaisaniemi", dist: "1.2" })).toBe(
      "Nearest weather station: Kaisaniemi (1.2 km)",
    );

    expect(t("en", "estimationFailed", { reason: "out of bounds" })).toBe("Estimation failed: out of bounds");
  });
});

describe("langOf", () => {
  it("prioritizes query param over browser languages", () => {
    expect(langOf("?lang=fi", ["en-US"])).toBe("fi");
    expect(langOf("?lang=en", ["fi-FI"])).toBe("en");
  });

  it("falls back to first recognized browser language when param is missing or invalid", () => {
    expect(langOf("", ["fi-FI", "en-US"])).toBe("fi");
    expect(langOf("", ["sv-SE", "en-US"])).toBe("sv");
    expect(langOf("?lang=sv", ["fi-FI"])).toBe("sv");
    expect(langOf("", ["en-GB", "fi-FI"])).toBe("en");
    expect(langOf("?lang=other", ["sv-FI"])).toBe("sv");
    expect(langOf("", ["de-DE", "fi-FI"])).toBe("fi");
    expect(langOf("", ["de-DE", "en-US"])).toBe("en");
    expect(langOf("", ["fr-FR"])).toBe("en");
    expect(langOf("", [])).toBe("en");
  });
});

describe("number formatting", () => {
  it("formats numbers according to language locale", () => {
    const en = number("en", 1234.56, 1);
    expect(en).toContain("1,234.6");

    const fi = number("fi", 1234.56, 1);
    expect(fi).toMatch(/1\s?234,6/);
  });

  it("formats integers without decimal places when requested", () => {
    expect(number("en", 42, 0)).toBe("42");
    expect(number("fi", 42, 0)).toBe("42");
  });
});

describe("formatHelsinkiTime", () => {
  it("formats timestamp into Europe/Helsinki local time", () => {
    // 2026-10-15T12:00:00Z is in summer time (EEST, UTC+3) -> 15:00
    const formatted = formatHelsinkiTime("2026-10-15T12:00:00Z", "en");
    expect(formatted).toBe("15:00");
  });

  it("handles invalid dates by returning empty string", () => {
    expect(formatHelsinkiTime("not-a-date", "en")).toBe("");
  });
});

describe("DAYS", () => {
  it("defines the seven days from Monday to Sunday", () => {
    expect(DAYS).toEqual(["mon", "tue", "wed", "thu", "fri", "sat", "sun"]);
  });
});
