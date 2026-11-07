/**
 * T-2755, UI-84: a big screen shows three form columns, and a field alone in its group takes the row.
 *
 * T-2713 wrote the third column as `min-[1800px]:grid-cols-3`. Tailwind emits an arbitrary
 * breakpoint before the named ones, so `md:grid-cols-2` came later in the stylesheet and won at
 * 2560 px: no form ever had three columns. The breakpoint is a named one now, and the guard below
 * keeps an arbitrary `min-[…px]` out of class names.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";

const SRC = join(__dirname, "../src");

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sources(path);
    return /\.tsx?$/.test(name) ? [path] : [];
  });
}

describe("form columns on a big screen (T-2755)", () => {
  it("defines the big screen as a named breakpoint", () => {
    expect(readFileSync(join(SRC, "index.css"), "utf8")).toMatch(/--breakpoint-3xl:\s*112\.5rem;/);
  });

  it("uses no arbitrary min-width breakpoint, which the stylesheet orders before md", () => {
    const arbitrary = sources(SRC).flatMap((path) =>
      [...readFileSync(path, "utf8").matchAll(/\bmin-\[\d+(?:px|rem)\]:/g)].map((m) => `${relative(SRC, path)}: ${m[0]}`),
    );
    expect(arbitrary).toEqual([]);
  });

  it("lays three scalar fields out in one, two and three columns, and a lone field across the row", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<Record<string, string>>
          schema={{
            type: "object",
            properties: {
              a: { type: "string", title: "Alpha" },
              b: { type: "string", title: "Beta" },
              c: { type: "string", title: "Gamma" },
              d: { type: "string", title: "Delta" },
            },
          }}
          uiSchema={{
            "ui:options": {
              groups: [
                { title: "Three", fields: ["a", "b", "c"] },
                { title: "One", fields: ["d"] },
              ],
            },
          }}
          formData={{}}
          onSubmit={vi.fn()}
        />
      </I18nextProvider>,
    );
    const cellOf = (label: string) => screen.getByLabelText(label).closest(".min-w-0") as HTMLElement;
    const grid = cellOf("Alpha").parentElement as HTMLElement;
    // The columns follow the card's width, not the screen's (T-4265): two from a 36rem card,
    // three from a 72rem one, in the container the grid sits in.
    expect(grid.parentElement?.className).toContain("@container");
    expect(grid.className).toContain("@xl:grid-cols-2");
    expect(grid.className).toContain("@6xl:grid-cols-3");
    expect(cellOf("Alpha").className).not.toContain("col-span-full");
    expect(cellOf("Delta").className).toContain("col-span-full");
  });
});
