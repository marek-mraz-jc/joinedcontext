/**
 * T-5122: the frame pieces a public page draws live outside Shell.tsx, so the catalogue's first
 * load does not pull the signed-in shell. LazyShell is what the router draws signed-in pages in;
 * vitest maps it to Shell itself (vite.config.ts), and scripts/bundle-budget.mjs proves the split
 * in the built entry.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Branding } from "../src/branding";
import { BrandMark, navLinkClass } from "../src/components/layout/BrandMark";
import { Shell as LazyShell } from "../src/components/layout/LazyShell";

// T-6891: a test sets the installation's branding fields; empty means the neutral block.
const override = vi.hoisted(() => ({ value: {} as Partial<Branding> }));
vi.mock("../src/branding", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../src/branding")>();
  return { ...actual, useBranding: () => ({ ...actual.useBranding(), ...override.value }) };
});
afterEach(() => {
  override.value = {};
});

describe("BrandMark and the lazy shell", () => {
  it("the_brand_mark_draws_the_jc_symbol_without_a_logo", () => {
    const { container } = render(<BrandMark />);
    const name = container.querySelector("span.truncate")?.textContent ?? "";
    expect(name.length).toBeGreaterThan(0);
    const symbol = screen.getByTestId("jc-symbol");
    expect(symbol.tagName.toLowerCase()).toBe("svg");
    expect(symbol).toHaveAttribute("aria-hidden", "true");
    expect(symbol.querySelectorAll("polygon").length).toBeGreaterThan(0);
    // The old fallback was the name's first letter in a square: no text beside the name now.
    expect(container.textContent).toBe(name);
    expect(container.querySelector("img")).toBeNull();
  });

  it("a_configured_logo_wins_over_the_symbol", () => {
    override.value = { logo: "logo.svg", instanceName: "Example City" };
    const { container } = render(<BrandMark />);
    expect(container.querySelector("img")).toHaveAttribute("src", "/api/v1/branding/logo");
    expect(screen.queryByTestId("jc-symbol")).toBeNull();
    // Below md the logo stands alone and the name stays for a screen reader (T-4165).
    expect(screen.getByText("Example City")).toHaveClass("max-md:sr-only");
  });

  it("two marks on one page keep their own mask ids", () => {
    const { container } = render(
      <>
        <BrandMark />
        <BrandMark short />
      </>,
    );
    const ids = [...container.querySelectorAll("mask, clipPath")].map((el) => el.id);
    expect(ids).toHaveLength(8);
    expect(new Set(ids).size).toBe(8);
  });

  it("the symbol is a static SVG of our own: no script, no external href, no foreignObject", () => {
    const source = readFileSync(join(__dirname, "../src/components/layout/BrandMark.tsx"), "utf8");
    expect(source).not.toMatch(/<script|foreignObject|href=|xlink|dangerouslySetInnerHTML|https?:\/\//i);
  });

  it("marks the active navigation link, the others stay plain", () => {
    expect(navLinkClass(true)).toContain("bg-primary-soft");
    expect(navLinkClass(false)).not.toContain("bg-primary-soft");
  });

  it("the router's shell is a component a test reads at once, not a pending lazy one", () => {
    expect(typeof LazyShell).toBe("function");
  });
});
