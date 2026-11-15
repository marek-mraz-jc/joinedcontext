import { describe, expect, it } from "vitest";
import { decimal, langOf, moment, number, t } from "./i18n";

describe("words and numbers", () => {
  it("takes the language from the address, else the browser's first of ours, else English", () => {
    expect(langOf("?lang=en", ["fi-FI"])).toBe("en");
    expect(langOf("?lang=sv", ["fi-FI"])).toBe("sv");
    expect(langOf("", ["sv-FI"])).toBe("sv");
    expect(langOf("", ["de-DE", "FI"])).toBe("fi");
    expect(langOf("?lang=de", ["de-DE"])).toBe("en");
  });

  it("writes areas, counts and moments the Finnish, Swedish and English way", () => {
    expect(decimal("sv", 2.46)).toBe("2,5");
    expect(moment("sv", Date.UTC(2030, 9, 7, 6, 5))).toBe("7.10.2030 09.05");
    expect(decimal("fi", 2.46)).toBe("2,5");
    expect(decimal("en", 9)).toBe("9.0");
    expect(number("fi", 1234)).toBe("1 234");
    expect(moment("fi", Date.UTC(2030, 9, 7, 6, 5))).toBe("7.10.2030 klo 09.05");
    expect(moment("en", Date.UTC(2030, 11, 7, 6, 5))).toBe("7 Dec 2030, 08:05");
  });

  it("fills its slots and leaves a missing one visible", () => {
    expect(t("en", "bandLegend", { minutes: 10 })).toBe("up to 10 min");
    expect(t("en", "bandLegend")).toBe("up to {minutes} min");
  });
});
