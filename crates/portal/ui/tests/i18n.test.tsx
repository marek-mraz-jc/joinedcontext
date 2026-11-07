import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import sk from "../src/locales/sk.json";
import en from "../src/locales/en.json";
import de from "../src/locales/de.json";
import cs from "../src/locales/cs.json";

function flattenKeys(obj: Record<string, unknown>, prefix = ""): string[] {
  let keys: string[] = [];
  for (const [key, value] of Object.entries(obj)) {
    const fullKey = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === "object" && !Array.isArray(value)) {
      keys = keys.concat(flattenKeys(value as Record<string, unknown>, fullKey));
    } else {
      keys.push(fullKey);
    }
  }
  return keys.sort();
}

type Bundle = {
  resourceDelete: { title: string; lead: string; propose: string; button: string; action: string };
  change: { summary: { delete: string } };
};

describe("i18n", () => {
  it("has identical flattened key sets across all four bundles", () => {
    const skKeys = flattenKeys(sk);
    const enKeys = flattenKeys(en);
    const deKeys = flattenKeys(de);
    const csKeys = flattenKeys(cs);

    expect(skKeys.length).toBeGreaterThan(0);
    expect(enKeys).toEqual(skKeys);
    expect(deKeys).toEqual(skKeys);
    expect(csKeys).toEqual(skKeys);
  });

  it("gives the German string for app.title after changeLanguage('de')", async () => {
    await i18n.changeLanguage("de");
    expect(i18n.t("app.title")).toBe(de.app.title);
  });

  // T-2804: English is the default, so a language the Portal lacks falls back to English.
  it("falls back to English for an unknown language", async () => {
    await i18n.changeLanguage("unknown-lang");
    expect(i18n.t("form.submit")).toBe(en.form.submit);
  });

  it("formats ICU plural of form.errors in Slovak for counts 0, 1, 2, and 5", async () => {
    await i18n.changeLanguage("sk");
    expect(i18n.t("form.errors", { count: 0 })).toBe("Bez chýb");
    expect(i18n.t("form.errors", { count: 1 })).toBe("1 chyba");
    expect(i18n.t("form.errors", { count: 2 })).toBe("2 chyby");
    expect(i18n.t("form.errors", { count: 5 })).toBe("5 chýb");
  });

  it("keeps document.documentElement.lang in step with the locale", async () => {
    await i18n.changeLanguage("de");
    expect(document.documentElement.lang).toBe("de");
    await i18n.changeLanguage("sk");
    expect(document.documentElement.lang).toBe("sk");
  });

  it("interpolates {name} in auth.signedInAs", async () => {
    await i18n.changeLanguage("sk");
    expect(i18n.t("auth.signedInAs", { name: "Alice" })).toBe("Prihlásený ako Alice");

    await i18n.changeLanguage("en");
    expect(i18n.t("auth.signedInAs", { name: "Alice" })).toBe("Signed in as Alice");
  });

  /**
   * One verb for one action (T-1425, UI-23). The row action opens the dialog, so the button that
   * opens it and every control inside it name the same act; a second verb makes a person ask
   * whether "remove" is softer than "delete". Nothing is removed until an approver confirms, so
   * the verb is the proposing one in each language.
   */
  describe("resourceDelete names the action with one verb per locale", () => {
    const VERB = {
      en: { keeps: /remov/i, rejects: /delet/i },
      sk: { keeps: /odstrán/i, rejects: /(zmaz|vymaz)/i },
      cs: { keeps: /odstran/i, rejects: /(smaz|vymaz)/i },
      de: { keeps: /entfern/i, rejects: /lösch/i },
    } as const;
    const BUNDLES = { en, sk, cs, de } as const;
    // close is "Close", referenced is a state, typeName repeats the name: only these five speak
    // the action itself. change.summary.delete is the same act read back in the approvals list,
    // so it says the same word as the button that proposed it.
    const SPOKEN = [
      ["resourceDelete.title", (b: Bundle) => b.resourceDelete.title],
      ["resourceDelete.lead", (b: Bundle) => b.resourceDelete.lead],
      ["resourceDelete.propose", (b: Bundle) => b.resourceDelete.propose],
      ["resourceDelete.button", (b: Bundle) => b.resourceDelete.button],
      ["resourceDelete.action", (b: Bundle) => b.resourceDelete.action],
      ["change.summary.delete", (b: Bundle) => b.change.summary.delete],
    ] as const;

    it.each(Object.keys(VERB) as (keyof typeof VERB)[])("%s", (locale) => {
      const { keeps, rejects } = VERB[locale];
      const bundle = BUNDLES[locale] as unknown as Bundle;
      for (const [path, read] of SPOKEN) {
        const text = read(bundle);
        expect(text, `${locale}.${path} is missing`).toBeTruthy();
        expect(keeps.test(text), `${locale}.${path} = ${text}`).toBe(true);
        expect(rejects.test(text), `${locale}.${path} = ${text}`).toBe(false);
      }
    });
  });
});
