// T-7706: endpoints/catalog.ts edges: which language an untranslated map shows, and the legislation list in the manifest.
import { describe, expect, it } from "vitest";
import { catalogOf, shownLanguage } from "../src/pages/endpoints/catalog";

describe("shownLanguage", () => {
  it("falls back to English, then to the first language held, then to the reader's", () => {
    expect(shownLanguage({ sk: "a", en: "b" }, "cs")).toBe("en");
    expect(shownLanguage({ sk: "a", de: "b" }, "cs")).toBe("sk");
    expect(shownLanguage({}, "cs")).toBe("cs");
    expect(shownLanguage(undefined, "cs")).toBe("cs");
    expect(shownLanguage({ cs: "x", en: "b" }, "cs")).toBe("cs");
  });
});

describe("catalogOf legislation", () => {
  it("writes the legislation the form lists, trimmed and without blanks, and none when empty", () => {
    const law = "https://eur-lex.europa.eu/eli/reg/2022/868/oj";
    expect(catalogOf({ applicableLegislation: [` ${law} `, "  "] }, undefined, "en")?.applicableLegislation).toEqual([law]);
    expect(catalogOf({ applicableLegislation: ["  "] }, undefined, "en")?.applicableLegislation).toBeUndefined();
  });
});
