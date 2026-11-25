// T-7706: i18n.ts: the grid's words load for sk, de and cs beside English, without the whole locale.
import { describe, expect, it } from "vitest";
import i18n, { i18nReady, loadGridLocales } from "../src/i18n";
import sk from "../src/locales/sk.json";
import de from "../src/locales/de.json";
import cs from "../src/locales/cs.json";

describe("loadGridLocales", () => {
  it.each([
    ["sk", sk.dataGrid.searchRows],
    ["de", de.dataGrid.searchRows],
    ["cs", cs.dataGrid.searchRows],
  ])("adds the grid's words of %s", async (language, searchRows) => {
    await i18nReady;
    await i18n.changeLanguage(language);
    await loadGridLocales();
    expect(i18n.getResource(language, "translation", "dataGrid.searchRows")).toBe(searchRows);
    expect(i18n.hasResourceBundle("en", "translation")).toBe(true);
    await i18n.changeLanguage("en");
  });
});
