import { afterEach, describe, expect, it, vi } from "vitest";
import type { Field } from "@joinedcontext/sdk";
import { parseInput } from "./components/EntityForm";
import { CATALOGS, language, pickLanguage, requestedLanguage, setLanguage, t } from "./i18n";

const placeholders = (text: string) => [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

afterEach(() => {
  setLanguage("en");
  vi.unstubAllGlobals();
  window.history.replaceState(null, "", "/");
});

describe("catalogs", () => {
  it("every language carries every English key with the same placeholders, and no other key", () => {
    const reference = CATALOGS.en;
    for (const [lang, catalog] of Object.entries(CATALOGS)) {
      expect(Object.keys(catalog).sort(), lang).toEqual(Object.keys(reference).sort());
      for (const [key, text] of Object.entries(catalog)) {
        expect(text.trim(), `${lang} ${key}`).not.toBe("");
        expect(placeholders(text), `${lang} ${key}`).toEqual(placeholders(reference[key as keyof typeof reference]));
      }
    }
  });
});

describe("pickLanguage", () => {
  it("takes the first tag whose language has a catalog, by its primary subtag", () => {
    expect(pickLanguage(["sk-SK", "de"])).toBe("sk");
    expect(pickLanguage(["fr-FR", "de_AT"])).toBe("de");
    expect(pickLanguage([" CS "])).toBe("cs");
  });

  it("answers English for nothing, empty tags and unknown languages", () => {
    expect(pickLanguage([])).toBe("en");
    expect(pickLanguage([null, undefined, "", "fr", "x-klingon"])).toBe("en");
    // A key of Object.prototype is no language.
    expect(pickLanguage(["constructor", "__proto__"])).toBe("en");
  });
});

describe("requestedLanguage", () => {
  it("prefers ?lang= over the browser, and the browser over the configuration", () => {
    vi.stubGlobal("navigator", { languages: ["de-DE", "en"], language: "de-DE" });
    window.history.replaceState(null, "", "/?lang=cs");
    expect(requestedLanguage("sk")).toBe("cs");
    window.history.replaceState(null, "", "/?lang=fr");
    expect(requestedLanguage("sk")).toBe("de");
    vi.stubGlobal("navigator", { languages: [], language: "fr" });
    expect(requestedLanguage("sk")).toBe("sk");
    expect(requestedLanguage(undefined)).toBe("en");
  });
});

describe("t", () => {
  it("answers in the language set, and sets the document's language for screen readers", () => {
    expect(t("table.page", { page: 1, pages: 3 })).toBe("Page 1 of 3");
    setLanguage("sk-SK");
    expect(language()).toBe("sk");
    expect(document.documentElement.lang).toBe("sk");
    expect(t("table.page", { page: 1, pages: 3 })).toBe("Strana 1 z 3");
    setLanguage("fr");
    expect(language()).toBe("en");
    expect(document.documentElement.lang).toBe("en");
  });

  it("leaves a placeholder no value is given for, and takes a value's text literally", () => {
    expect(t("table.page", { page: 2 })).toBe("Page 2 of {pages}");
    expect(t("form.notInList", { value: "{pages} $& $1" })).toBe("{pages} $& $1 (not in the list)");
    expect(t("filter.from", { label: "constructor" })).toBe("constructor from");
    expect(t("state.retry", { toString: "x" })).toBe("Retry");
  });

  it("puts the form's refusals into the person's language", () => {
    const field: Field = { name: "count", input: "number", min: 0, max: 10, required: false };
    setLanguage("de");
    expect(parseInput(field, "abc")).toEqual({ error: "muss eine Zahl sein" });
    expect(parseInput(field, "11")).toEqual({ error: "darf höchstens 10 sein" });
    setLanguage("cs");
    expect(parseInput(field, "-1")).toEqual({ error: "musí být alespoň 0" });
  });
});
