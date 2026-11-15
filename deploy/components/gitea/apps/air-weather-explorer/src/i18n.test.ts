import { describe, expect, it } from "vitest";
import { langOf, number } from "./i18n";
import { t } from "./texts";

describe("i18n", () => {
  it("takes the address's language, then the Portal's, then the browser's, then Finnish", () => {
    expect(langOf("?lang=en", "fi", "fi-FI")).toBe("en");
    expect(langOf("?lang=de", "en", "fi-FI")).toBe("en");
    expect(langOf("", undefined, "en-GB")).toBe("en");
    expect(langOf("", "sv", "en-GB")).toBe("sv");
    expect(langOf("?lang=de", "de", "de-DE")).toBe("fi");
  });

  it("writes numbers as Finland does, in any of its languages", () => {
    expect(number(1234.5, 1)).toBe("1\u00a0234,5");
    expect(number(-0.25, 1)).toBe("−0,3");
    expect(number(-0.004, 2)).toBe("0,00");
  });

  it("fills a text's names and leaves an unknown one as it is", () => {
    expect(t("fi", "days", { n: 3 })).toBe("3 vrk");
    expect(t("en", "days")).toBe("{n} days");
  });
});
