import { describe, expect, it } from "vitest";
import { applyTokens, currentTokens, DEFAULT_TOKENS, echartsTheme, resolveTokens, withBundledFont } from "../src/sdk/tokens";

describe("sdk tokens", () => {
  it("defaults equal the index.css :root colours", () => {
    expect(DEFAULT_TOKENS.color.accent).toBe("#0f766e");
    expect(DEFAULT_TOKENS.color.ink).toBe("#0f172a");
    expect(DEFAULT_TOKENS.color.muted).toBe("#64748b");
    expect(DEFAULT_TOKENS.color.surface).toBe("#ffffff");
    expect(DEFAULT_TOKENS.color.card).toBe("#f8fafc");
    expect(DEFAULT_TOKENS.color.line).toBe("#e2e8f0");
    expect(DEFAULT_TOKENS.chart.palette).toContain("#0f766e");
  });

  it("draws the bundled Inter before any system family, whatever stack the app names (T-0756)", () => {
    expect(DEFAULT_TOKENS.font.body.startsWith('"Inter"')).toBe(true);
    expect(withBundledFont("Georgia, serif")).toBe('Georgia, "Inter", serif');
    expect(withBundledFont("system-ui, sans-serif")).toBe('"Inter", system-ui, sans-serif');
    expect(withBundledFont('"Inter", sans-serif')).toBe('"Inter", sans-serif');
    const root = document.createElement("div");
    applyTokens(resolveTokens({ font: { body: "system-ui, sans-serif" } }).tokens, root);
    expect(root.style.getPropertyValue("--jc-font-body")).toBe('"Inter", system-ui, sans-serif');
  });

  it("merges partial override over defaults", () => {
    const { tokens, warnings } = resolveTokens({
      color: { accent: "#2563eb" },
    });
    expect(warnings).toHaveLength(0);
    expect(tokens.color.accent).toBe("#2563eb");
    expect(tokens.color.ink).toBe(DEFAULT_TOKENS.color.ink);
  });

  it("drops wrong type and unsafe CSS values with warnings", () => {
    const { tokens, warnings } = resolveTokens({
      color: {
        accent: "red; background:url(x)", // unsafe semicolon and url
        ink: 12345, // wrong type
      },
      unknownRoot: "hello",
    });

    expect(warnings.length).toBeGreaterThanOrEqual(3);
    expect(warnings).toContain("unknownRoot: unknown property");
    expect(tokens.color.accent).toBe(DEFAULT_TOKENS.color.accent);
    expect(tokens.color.ink).toBe(DEFAULT_TOKENS.color.ink);
  });

  it("applyTokens sets both --jc-color-accent and legacy --accent on target element", () => {
    const el = document.createElement("div");
    applyTokens({ color: { accent: "#7c3aed" } }, el);

    expect(el.style.getPropertyValue("--jc-color-accent")).toBe("#7c3aed");
    expect(el.style.getPropertyValue("--accent")).toBe("#7c3aed");
    expect(currentTokens().color.accent).toBe("#7c3aed");
  });

  it("echartsTheme uses the chart palette and textStyle", () => {
    const theme = echartsTheme();
    expect(theme.color).toEqual(DEFAULT_TOKENS.chart.palette);
    expect(theme.backgroundColor).toBe("transparent");
    expect(theme.textStyle).toEqual({
      color: DEFAULT_TOKENS.color.ink,
      fontFamily: DEFAULT_TOKENS.font.body,
    });
  });
});
