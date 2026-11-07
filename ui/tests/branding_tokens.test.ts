/**
 * T-1728: what may reach a colour or font token (PF-50, UI-01).
 *
 * `applyBranding` writes CSS custom properties, and a custom property accepts almost any token
 * stream — `url(...)` in one is fetched the moment something reads it. The branding answer comes
 * from a file an organisation administrator writes, so every value is checked here as well as at
 * the API: this is the side that still holds when the answer did not come from our own handler.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { applyBranding, isFontStack, isHexColour, withBundledFont } from "../src/branding";
import type { Branding } from "../src/branding";

const base: Branding = {
  instanceName: "Helsinki Context",
  primaryForeground: "#ffffff",
} as Branding;

function apply(branding: Partial<Branding>) {
  const root = document.documentElement;
  root.removeAttribute("style");
  applyBranding({ ...base, ...branding } as Branding, document);
  return root.style;
}

beforeEach(() => {
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

describe("what a colour token takes", () => {
  it("a_brand_value_that_is_not_a_colour_is_ignored", () => {
    for (const bad of [
      "url(https://elsewhere.example/pixel)",
      "red; background: url(https://elsewhere.example/x)",
      "var(--something-else)",
      "#12345",
      "#ggg",
      "rgb(1,2,3)",
      "expression(alert(1))",
      "  #fff  ",
    ]) {
      const style = apply({ colours: { primary: bad } } as Partial<Branding>);
      expect(style.getPropertyValue("--portal-color-primary"), bad).toBe("");
    }
  });

  it("a_hex_colour_reaches_the_token", () => {
    expect(apply({ colours: { primary: "#1d4ed8" } } as Partial<Branding>).getPropertyValue(
      "--portal-color-primary",
    )).toBe("#1d4ed8");
    expect(apply({ colours: { primary: "#fff" } } as Partial<Branding>).getPropertyValue(
      "--portal-color-primary",
    )).toBe("#fff");
  });

  it("the_foreground_the_server_computed_is_checked_like_any_other_value", () => {
    expect(apply({ primaryForeground: "url(x)" }).getPropertyValue("--portal-color-primary-fg")).toBe(
      "",
    );
    expect(apply({ primaryForeground: "#0f172a" }).getPropertyValue("--portal-color-primary-fg")).toBe(
      "#0f172a",
    );
  });

  it("the_dark_themes_pair_is_written_and_checked_like_the_light_one", () => {
    // Both are computed by the API and both land in a custom property, so both are checked here
    // as well: an answer that did not come from our own handler is still an answer (T-2324).
    const good = apply({ primaryDark: "#4a505d", primaryForegroundDark: "#ffffff" });
    expect(good.getPropertyValue("--portal-color-primary-dark")).toBe("#4a505d");
    expect(good.getPropertyValue("--portal-color-primary-fg-dark")).toBe("#ffffff");

    const bad = apply({ primaryDark: "url(x)", primaryForegroundDark: "expression(alert(1))" });
    expect(bad.getPropertyValue("--portal-color-primary-dark")).toBe("");
    expect(bad.getPropertyValue("--portal-color-primary-fg-dark")).toBe("");
  });

  it("isHexColour_takes_three_and_six_digits_and_nothing_else", () => {
    for (const good of ["#fff", "#FFF", "#1d4ed8", "#1D4ED8"]) expect(isHexColour(good), good).toBe(true);
    for (const bad of ["fff", "#ff", "#ffff", "#fffffff", "#ggg", "", "#", "#fff "])
      expect(isHexColour(bad), bad).toBe(false);
  });
});

describe("what a font token takes", () => {
  it("a_font_stack_that_could_fetch_something_is_ignored", () => {
    for (const bad of [
      'Inter, url("https://elsewhere.example/f.woff")',
      "Inter; background: red",
      "local(Inter)",
      "Inter/**/",
      "@import url(x)",
      "x".repeat(201),
    ]) {
      const style = apply({ fonts: { body: bad } } as Partial<Branding>);
      expect(style.getPropertyValue("--portal-font-sans"), bad).toBe("");
    }
  });

  it("a_family_list_reaches_the_token_with_the_bundled_face_in_it", () => {
    const style = apply({ fonts: { body: "Helvetica, sans-serif" } } as Partial<Branding>);
    expect(style.getPropertyValue("--portal-font-sans")).toBe(withBundledFont("Helvetica, sans-serif"));
    expect(style.getPropertyValue("--portal-font-sans")).toContain('"Inter"');
  });

  it("isFontStack_takes_family_names_and_the_commas_between_them", () => {
    for (const good of ['Inter, "Noto Sans", sans-serif', "Helvetica", "IBM Plex Sans, serif"])
      expect(isFontStack(good), good).toBe(true);
    for (const bad of ["url(x)", "a(b)", "a;b", "a/b", "@font-face", ""])
      expect(isFontStack(bad), bad).toBe(false);
  });
});

describe("what is always written", () => {
  it("the_title_is_the_instance_name_and_never_a_style", () => {
    apply({ instanceName: "Brno Context" });
    expect(document.title).toBe("Brno Context");
  });

  it("a_branding_with_nothing_in_it_leaves_every_token_at_its_default", () => {
    const style = apply({});
    for (const token of [
      "--portal-color-primary",
      "--portal-color-secondary",
      "--portal-color-accent",
      "--portal-color-surface",
      "--portal-font-sans",
      "--portal-font-heading",
    ]) {
      expect(style.getPropertyValue(token), token).toBe("");
    }
  });
});
