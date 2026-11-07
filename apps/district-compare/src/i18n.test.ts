import { describe, expect, it } from "vitest";
import { MEASURES } from "./districts";
import type { Lang, Word } from "./i18n";
import { langOf, measureTitle, measureUnit, number, t, translateType } from "./i18n";

const ALL_WORDS: Word[] = [
  "title",
  "page",
  "measure",
  "districtsRanked",
  "selectedDistrictsCompared",
  "measure_events",
  "measure_bikes",
  "measure_bikeSlots",
  "measure_alerts",
  "measure_pm25",
  "measure_aqi",
  "area",
  "km2",
  "perKm2",
  "ugm3",
  "noStation",
  "unlocatedEvents",
  "emptyDistricts",
  "loading",
  "analysing",
  "unreadable",
  "unreadableStatus",
  "partialFailed",
  "analysisFailed",
  "clear",
  "details",
  "map",
  "mapLegend",
  "chartTitle",
  "chartEmpty",
  "noneSelected",
  "deselectDistrict",
  "selectDistrict",
  "type_AdministrativeArea",
  "type_Event",
  "type_BikeHireDockingStation",
  "type_Alert",
  "type_AirQualityObserved",
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
    expect(t("en", "unlocatedEvents", { n: 4 })).toBe("4 events without a place.");
    expect(t("fi", "unlocatedEvents", { n: 4 })).toBe("4 tapahtumaa ilman sijaintitietoa.");

    expect(t("en", "unreadableStatus", { status: 503 })).toBe("Data could not be read (HTTP 503). Try again later.");
    expect(t("fi", "unreadableStatus", { status: 503 })).toBe(
      "Tietoja ei voitu lukea (HTTP 503). Yritä myöhemmin uudelleen.",
    );

    expect(t("en", "chartTitle", { measure: "Events" })).toBe("Selected districts: Events");
    expect(t("fi", "chartTitle", { measure: "Tapahtumat" })).toBe("Valitut kaupunginosat: Tapahtumat");

    expect(t("en", "deselectDistrict", { name: "Kallio" })).toBe("Deselect Kallio");
    expect(t("fi", "deselectDistrict", { name: "Kallio" })).toBe("Poista valinta: Kallio");
  });
});

describe("langOf", () => {
  it("prioritizes query param over browser languages", () => {
    expect(langOf("?lang=fi", ["en-US"])).toBe("fi");
    expect(langOf("?lang=en", ["fi-FI"])).toBe("en");
  });

  it("detects Finnish or Swedish from browser languages when param is missing or invalid", () => {
    expect(langOf("", ["fi-FI"])).toBe("fi");
    expect(langOf("", ["sv-SE"])).toBe("sv");
    expect(langOf("?lang=sv", ["fi-FI"])).toBe("sv");
    expect(langOf("?lang=invalid", ["sv-FI"])).toBe("sv");
    expect(langOf("", ["en-GB", "fi"])).toBe("en");
    expect(langOf("", ["de-DE"])).toBe("en");
    expect(langOf("", [])).toBe("en");
  });
});

describe("number formatting", () => {
  it("formats integers and decimals according to locale", () => {
    const formattedEn = number("en", 1234.56, 2);
    expect(formattedEn).toContain("1,234.56");

    const formattedFi = number("fi", 1234.56, 2);
    expect(formattedFi).toMatch(/1\s?234,56/);
  });
});

describe("measure titles and units", () => {
  it("provides non-empty titles for all measures", () => {
    const langs: Lang[] = ["en", "fi"];
    for (const l of langs) {
      for (const m of MEASURES) {
        const title = measureTitle(l, m);
        expect(title.trim().length).toBeGreaterThan(0);
      }
    }
  });

  it("returns correct units for measures", () => {
    expect(measureUnit("en", "pm25")).toBe("µg/m³");
    expect(measureUnit("fi", "pm25")).toBe("µg/m³");
    expect(measureUnit("en", "events")).toBe("");
    expect(measureUnit("fi", "events")).toBe("");
  });
});

describe("translateType", () => {
  it("translates known entity types and falls back to raw type name for unknown", () => {
    expect(translateType("AdministrativeArea", "en")).toBe("City districts");
    expect(translateType("AdministrativeArea", "fi")).toBe("Kaupunginosat");
    expect(translateType("Event", "en")).toBe("Events");
    expect(translateType("Event", "fi")).toBe("Tapahtumat");
    expect(translateType("BikeHireDockingStation", "en")).toBe("Bike stations");
    expect(translateType("BikeHireDockingStation", "fi")).toBe("Kaupunkipyöräasemat");
    expect(translateType("Alert", "en")).toBe("Alerts");
    expect(translateType("Alert", "fi")).toBe("Liikennetiedotteet");
    expect(translateType("AirQualityObserved", "en")).toBe("Air quality");
    expect(translateType("AirQualityObserved", "fi")).toBe("Ilmanlaatu");

    expect(translateType("CustomEntity", "en")).toBe("CustomEntity");
  });
});
