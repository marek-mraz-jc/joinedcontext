/**
 * T-4264, T-4160 #1 #2: a kind reaches the screen as the page language's words, never as the
 * CamelCase name a manifest carries.
 */
import { afterAll, describe, expect, it } from "vitest";
import i18n, { i18nReady } from "../src/i18n";
import { kindNew, kindPlural } from "../src/kindWords";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";

const t = (key: string, options?: Record<string, unknown>) => i18n.t(key, options);

afterAll(async () => {
  await i18n.changeLanguage("en");
});

describe("kind words (T-4264)", () => {
  it("names the create form and the plural in English", async () => {
    await i18nReady;
    await i18n.changeLanguage("en");
    expect(kindNew("KnowledgeSource", t)).toBe("New knowledge source");
    expect(kindPlural("ContextSpace", t)).toBe("Context Spaces");
    expect(t("form.guideLink", { kind: kindPlural("App", t) })).toBe("Read about applications in the User Guide");
  });

  it("names them in Slovak, each with its own gender", async () => {
    await i18n.changeLanguage("sk");
    expect(kindNew("KnowledgeSource", t)).toBe("Nový zdroj znalostí");
    expect(kindNew("AssistantDeployment", t)).toBe("Nové nasadenie asistenta");
    expect(kindNew("App", t)).toBe("Nová aplikácia");
    expect(kindPlural("ContextSpace", t)).toBe("kontextové priestory");
  });

  it("reads a kind no catalogue holds yet as words, not CamelCase", async () => {
    await i18n.changeLanguage("en");
    expect(kindPlural("FutureThing", t)).toBe("future thing");
    expect(kindNew("FutureThing", t)).toBe("New future thing");
  });

  it("holds the same kinds, each with both forms, in every catalogue", () => {
    const kinds = Object.keys(en.kinds).sort();
    for (const catalogue of [sk, cs, de]) {
      expect(Object.keys(catalogue.kinds).sort()).toEqual(kinds);
      for (const words of Object.values(catalogue.kinds)) {
        expect(words.other.trim()).not.toBe("");
        expect(words.new.trim()).not.toBe("");
      }
    }
  });
});
