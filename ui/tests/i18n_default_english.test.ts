// T-2804: English is the default; another language only when a person picks one.
import { describe, expect, it } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";

describe("default language", () => {
  it("falls back to English", () => {
    expect(i18n.options.fallbackLng).toEqual(["en"]);
    expect(SUPPORTED_LOCALES[0]).toBe("en");
  });

  it("never takes the browser's language on its own", () => {
    const order = (i18n.options.detection as { order?: string[] } | undefined)?.order ?? [];
    expect(order).not.toContain("navigator");
    expect(order).toEqual(["querystring", "localStorage"]);
  });
});
