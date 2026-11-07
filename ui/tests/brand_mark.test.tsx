/**
 * T-5122: the frame pieces a public page draws live outside Shell.tsx, so the catalogue's first
 * load does not pull the signed-in shell. LazyShell is what the router draws signed-in pages in;
 * vitest maps it to Shell itself (vite.config.ts), and scripts/bundle-budget.mjs proves the split
 * in the built entry.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { BrandMark, navLinkClass } from "../src/components/layout/BrandMark";
import { Shell as LazyShell } from "../src/components/layout/LazyShell";

describe("BrandMark and the lazy shell", () => {
  it("draws the instance's initial beside its name without the shell", () => {
    const { container } = render(<BrandMark />);
    const name = container.querySelector("span.truncate")?.textContent ?? "";
    expect(name.length).toBeGreaterThan(0);
    expect(screen.getByText(name.trim().charAt(0).toUpperCase())).toHaveAttribute("aria-hidden", "true");
  });

  it("marks the active navigation link, the others stay plain", () => {
    expect(navLinkClass(true)).toContain("bg-primary-soft");
    expect(navLinkClass(false)).not.toContain("bg-primary-soft");
  });

  it("the router's shell is a component a test reads at once, not a pending lazy one", () => {
    expect(typeof LazyShell).toBe("function");
  });
});
