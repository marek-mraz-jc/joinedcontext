import { afterEach, describe, expect, it, vi } from "vitest";
import { formatDate, getLanguage, t } from "./i18n";

describe("getLanguage", () => {
  afterEach(() => {
    window.history.replaceState(null, "", "/");
    vi.unstubAllGlobals();
  });

  it("takes ?lang= from the address, then from its #topics part, then the browser's, else English", () => {
    window.history.replaceState(null, "", "/?lang=fi");
    expect(getLanguage()).toBe("fi");
    window.history.replaceState(null, "", "/?lang=xx#topics?lang=en");
    expect(getLanguage()).toBe("en");
    window.history.replaceState(null, "", "/?lang=sv");
    expect(getLanguage()).toBe("sv");
    window.history.replaceState(null, "", "/#topics");
    vi.stubGlobal("navigator", { language: "fi-FI" });
    expect(getLanguage()).toBe("fi");
    vi.stubGlobal("navigator", { language: "sv-SE" });
    expect(getLanguage()).toBe("sv");
    vi.stubGlobal("navigator", { language: "de-DE" });
    expect(getLanguage()).toBe("en");
  });
});

describe("formatDate", () => {
  it("writes a date in Helsinki's time, and nothing for no date or one it cannot read", () => {
    expect(formatDate("2026-10-05T08:00:00Z", "en")).toContain("11:00");
    expect(formatDate(new Date("2026-10-05T08:00:00Z"), "fi")).toContain("11.00");
    expect(formatDate(null, "en")).toBe("");
    expect(formatDate("soon", "en")).toBe("");
    expect(formatDate("2026-10-05T08:00:00Z", "sv")).toContain("11.00");
  });
});

describe("t", () => {
  it("fills its slots and leaves a missing one visible", () => {
    expect(t("follow", "en", { name: "kirjasto" })).toBe("Follow kirjasto");
    expect(t("nameLine", "sv", { term: "kirjasto", name: "Bibliotek", who: "Sara" })).toBe("kirjasto: Bibliotek, av Sara");
    expect(t("follow", "fi")).toBe("Seuraa: {name}");
  });
});
