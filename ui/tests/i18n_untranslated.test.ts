// T-2801: a value in sk, cs or de that equals English is untranslated unless the allow list says
// the language keeps that word. Placeholders ({name}) are not words; a value made of them alone
// and punctuation needs no translation.
import { describe, expect, it } from "vitest";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";
import allow from "./i18n_same_as_english.allow.json";

type Bundle = { [key: string]: string | Bundle };

function flatten(bundle: Bundle, prefix = ""): Map<string, string> {
  const out = new Map<string, string>();
  for (const [key, value] of Object.entries(bundle)) {
    if (typeof value === "string") out.set(prefix + key, value);
    else for (const [k, v] of flatten(value, `${prefix}${key}.`)) out.set(k, v);
  }
  return out;
}

const hasWords = (value: string) => /[A-Za-z]/.test(value.replace(/\{\w+\}/g, ""));

/** Keys whose value equals English and is not allowed, and allowed words no value uses. */
function untranslated(english: Bundle, bundle: Bundle, allowed: string[]) {
  const source = flatten(english);
  const same = [...flatten(bundle)].filter(([key, value]) => source.get(key) === value && hasWords(value));
  const kept = new Set(allowed);
  return {
    keys: same.filter(([, value]) => !kept.has(value)).map(([key, value]) => `${key}: ${value}`),
    stale: allowed.filter((word) => !same.some(([, value]) => value === word)),
  };
}

const bundles: Record<"sk" | "cs" | "de", Bundle> = { sk, cs, de };

describe("every language is translated", () => {
  for (const [lang, bundle] of Object.entries(bundles) as ["sk" | "cs" | "de", Bundle][]) {
    it(`${lang} says nothing in English it should not`, () => {
      const { keys, stale } = untranslated(en, bundle, allow[lang]);
      expect(keys, `translate these in src/locales/${lang}.json`).toEqual([]);
      expect(stale, `no ${lang} value uses these any more: drop them from the allow list`).toEqual([]);
    });
  }

  it("goes red on an English value and on a stale allow entry", () => {
    const english = { a: { save: "Save" }, count: "{n} × {what}", file: "JSON" };
    const result = untranslated(english, { a: { save: "Save" }, count: "{n} × {what}", file: "JSON" }, ["JSON", "Menu"]);
    expect(result).toEqual({ keys: ["a.save: Save"], stale: ["Menu"] });
  });
});
