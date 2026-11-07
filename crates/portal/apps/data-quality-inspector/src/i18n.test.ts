import { describe, expect, it } from "vitest";
import type { Word } from "./i18n";
import { ageMessage, langOf, number, ruleMessage, t } from "./i18n";

const ALL_WORDS: Word[] = [
  "title",
  "page",
  "typesList",
  "attributesTable",
  "failingTable",
  "loading",
  "loadingProgress",
  "analysing",
  "schemaUnreadable",
  "typeReadError",
  "allTypesFailed",
  "noSchema",
  "noTimestamp",
  "entities",
  "completeness",
  "valid",
  "medianAge",
  "oldest",
  "freshness",
  "freshnessField",
  "olderThan24hTitle",
  "olderThan24h",
  "chartTitle",
  "chartEmpty",
  "overview",
  "allTypes",
  "attribute",
  "required",
  "notChecked",
  "yes",
  "no",
  "id",
  "failedRule",
  "failingCapped",
  "allValid",
  "noEntities",
  "noSchemaDetails",
  "inspectFailed",
  "rule_datetime",
  "rule_date",
  "rule_uri",
  "rule_minimum",
  "rule_maximum",
  "rule_enum",
  "rule_required",
  "rule_unknown",
  "rule_pattern",
  "rule_type",
  "rule_format_other",
];

describe("i18n words", () => {
  it("every key exists in en and fi and none is empty", () => {
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
    expect(t("fi", "loadingProgress", { done: 3, total: 10 })).toBe("Luetaan 3 / 10 tyypistä…");
    expect(t("en", "loadingProgress", { done: 3, total: 10 })).toBe("Reading 3 of 10 types…");

    expect(t("en", "failingCapped", { shown: 200, total: 250 })).toBe(
      "Showing first 200 of 250 failing entities.",
    );
    expect(t("fi", "failingCapped", { shown: 200, total: 250 })).toBe(
      "Näytetään ensimmäiset 200 / 250 virheellisestä kohteesta.",
    );

    expect(t("en", "inspectFailed", { reason: "out of memory" })).toBe(
      "Inspection failed: out of memory",
    );
    expect(t("fi", "inspectFailed", { reason: "muisti loppui" })).toBe(
      "Tarkastus epäonnistui: muisti loppui",
    );

    expect(t("en", "attributesTable", { type: "BikeStation" })).toBe("Attributes of BikeStation");
    expect(t("fi", "attributesTable", { type: "BikeStation" })).toBe("Tyypin BikeStation ominaisuudet");
  });
});

describe("langOf", () => {
  it("prioritizes query param when set to valid language", () => {
    expect(langOf("?lang=fi", ["en-US"])).toBe("fi");
    expect(langOf("?lang=en", ["fi-FI"])).toBe("en");
  });

  it("first language in browser preference decides between fi/sv and en", () => {
    // English first
    expect(langOf("", ["en-GB", "fi-FI"])).toBe("en");
    expect(langOf("", ["en-US", "sv-SE"])).toBe("en");

    // Finnish first
    expect(langOf("", ["fi-FI", "en-GB"])).toBe("fi");

    // Swedish first -> Swedish, which the App speaks (T-3827)
    expect(langOf("", ["sv-FI", "en-US"])).toBe("sv");
    expect(langOf("", ["sv-SE", "en-GB"])).toBe("sv");
    expect(langOf("?lang=sv", ["fi-FI"])).toBe("sv");

    // Unsupported languages before supported language
    expect(langOf("", ["de-DE", "fi-FI", "en-US"])).toBe("fi");
    expect(langOf("", ["de-DE", "en-US", "fi-FI"])).toBe("en");

    // No match or empty defaults to en
    expect(langOf("", ["de-DE", "fr-FR"])).toBe("en");
    expect(langOf("", [])).toBe("en");

    // Invalid query param falls back to browser preferences
    expect(langOf("?lang=es", ["fi-FI"])).toBe("fi");
    expect(langOf("?lang=invalid", ["en-US"])).toBe("en");
  });
});

describe("number formatting", () => {
  it("formats integers and decimals according to locale", () => {
    const en = number("en", 1234.56, 2);
    expect(en).toContain("1,234.56");

    const fi = number("fi", 1234.56, 2);
    expect(fi).toMatch(/1\s?234,56/);

    expect(number("en", 100)).toBe("100");
    expect(number("fi", 100)).toBe("100");
  });
});

describe("ageMessage", () => {
  it("formats durations into human words in both languages", () => {
    expect(ageMessage("en", 45)).toBe("45 s");
    expect(ageMessage("fi", 45)).toBe("45 s");

    expect(ageMessage("en", 300)).toBe("5 min");
    expect(ageMessage("fi", 300)).toBe("5 min");

    expect(ageMessage("en", 7200)).toBe("2 h");
    expect(ageMessage("fi", 7200)).toBe("2 h");

    expect(ageMessage("en", 86400)).toBe("1 day");
    expect(ageMessage("fi", 86400)).toBe("1 pv");

    expect(ageMessage("en", 259200)).toBe("3 days");
    expect(ageMessage("fi", 259200)).toBe("3 pv");

    expect(ageMessage("en", -10)).toBe("0 min");
    expect(ageMessage("fi", Number.NaN)).toBe("0 min");
  });
});

describe("ruleMessage", () => {
  it("words rules into natural phrasing in both languages", () => {
    expect(ruleMessage("en", "format", "date-time")).toBe("not a date-time");
    expect(ruleMessage("fi", "format", "date-time")).toBe("ei kelvollinen aikaleima");

    expect(ruleMessage("en", "format", "date")).toBe("not a date");
    expect(ruleMessage("fi", "format", "date")).toBe("ei kelvollinen päivämäärä");

    expect(ruleMessage("en", "format", "uri")).toBe("not a URI");
    expect(ruleMessage("fi", "format", "uri")).toBe("ei kelvollinen URI");

    expect(ruleMessage("en", "minimum", "10")).toBe("below the minimum 10");
    expect(ruleMessage("fi", "minimum", "10")).toBe("alle minimin 10");

    expect(ruleMessage("en", "maximum", "100")).toBe("above the maximum 100");
    expect(ruleMessage("fi", "maximum", "100")).toBe("yli maksimin 100");

    expect(ruleMessage("en", "enum", "on, off")).toBe("not one of: on, off");
    expect(ruleMessage("fi", "enum", "on, off")).toBe("ei mikään näistä: on, off");

    expect(ruleMessage("en", "required", "")).toBe("missing, but required");
    expect(ruleMessage("fi", "required", "")).toBe("puuttuu, mutta pakollinen");

    expect(ruleMessage("en", "unknown", "")).toBe("not in the model");
    expect(ruleMessage("fi", "unknown", "")).toBe("ei mallissa");

    expect(ruleMessage("en", "pattern", "^[0-9]+$")).toBe("does not match pattern ^[0-9]+$");
    expect(ruleMessage("fi", "pattern", "^[0-9]+$")).toBe("ei vastaa säännöllistä lauseketta ^[0-9]+$");

    expect(ruleMessage("en", "type", "integer")).toBe("not of type integer");
    expect(ruleMessage("fi", "type", "integer")).toBe("ei tyyppiä integer");
  });
});
