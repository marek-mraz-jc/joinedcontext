// covers src/components/forms/widgets/AppearanceWidgets.tsx: the colour swatch and the SVG preview
// of an Organization's appearance form (UIX-16, T-6772).
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Swatch, svgImageUrl } from "../src/components/forms/widgets/AppearanceWidgets";

describe("the appearance form's widgets (T-6772)", () => {
  it("previews pasted SVG only as a data URL an <img> draws, and nothing else", () => {
    const url = svgImageUrl(' <svg viewBox="0 0 1 1"><rect/></svg>');
    expect(url).toMatch(/^data:image\/svg\+xml;charset=utf-8,%3Csvg/);
    expect(svgImageUrl("<html><script>x()</script></html>")).toBeUndefined();
    expect(svgImageUrl(undefined)).toBeUndefined();
  });

  it("paints a hex colour, a dashed square when unset and a struck red one for a value that is no colour", () => {
    const rect = (colour?: string) => render(<Swatch colour={colour} className="h-4" />).container.querySelector("rect");
    expect(rect("#0b5394")?.getAttribute("fill")).toBe("#0b5394");
    const refused = render(<Swatch colour="red;}body{x}" className="h-4" />).container;
    expect(refused.querySelector("rect")?.getAttribute("fill")).toBeNull();
    expect(refused.querySelector("line")).not.toBeNull();
    expect(render(<Swatch className="h-4" />).container.querySelector("svg")?.getAttribute("class")).toContain("border-dashed");
  });
});
