import { describe, expect, it } from "vitest";
import { langOf, localeOf, number } from "./i18n";
import { t } from "./texts";

describe("i18n", () => {
  it("takes the address's language, then the Portal's, then the browser's, then Finnish", () => {
    expect(langOf("?lang=en", "fi", "fi-FI")).toBe("en");
    expect(langOf("?lang=de", "en", "fi-FI")).toBe("en");
    expect(langOf("", undefined, "en-GB")).toBe("en");
    expect(langOf("", "sv", "sv-SE")).toBe("sv");
    expect(langOf("?lang=de", undefined, "de-DE")).toBe("fi");
  });

  it("writes numbers as Finland does, in either language", () => {
    expect(number(1234.5, 1)).toBe("1\u00a0234,5");
    expect(number(-0.25, 1)).toBe("−0,3");
    expect(number(-0.004, 2)).toBe("0,00");
  });

  it("writes dates as Finland does in each language", () => {
    expect(["fi", "en", "sv"].map((lang) => localeOf(lang as "fi"))).toEqual(["fi-FI", "en-FI", "sv-FI"]);
  });

  it("fills a text's names and leaves an unknown one as it is", () => {
    expect(t("fi", "pick", { n: 3 })).toBe("Ota 3");
    expect(t("en", "pick")).toBe("Take {n}");
    expect(t("sv", "pick", { n: 2 })).toBe("Ta 2");
  });
});
