import { describe, expect, it } from "vitest";
import i18n, { i18nReady, localeLoader } from "../src/i18n";
import sk from "../src/locales/sk.json";

// T-3316: English ships in the entry, every other language loads when it is chosen.
describe("locales load on demand", () => {
  it("holds English only until another language is chosen", async () => {
    await i18nReady;
    expect(i18n.hasResourceBundle("en", "translation")).toBe(true);
    expect(i18n.hasResourceBundle("de", "translation")).toBe(false);
    await i18n.changeLanguage("de");
    expect(i18n.hasResourceBundle("de", "translation")).toBe(true);
    expect(i18n.t("app.loading")).not.toBe("app.loading");
    await i18n.changeLanguage("en");
  });

  it("reads a language's own file and nothing for a language it does not ship", async () => {
    const read = (language: string) =>
      new Promise<unknown>((resolve) => localeLoader.read(language, "translation", (_error, data) => resolve(data)));
    expect(await read("sk")).toEqual(sk);
    expect(await read("fr")).toEqual({});
  });
});
