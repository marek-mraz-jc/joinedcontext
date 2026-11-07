/**
 * The Baserow-shaped grid (T-4883): the row number column with its checkbox and expand button,
 * shift-click ranges, no filter row for a host with its own panel, the footer and the row size.
 */
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { useState } from "react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import type { FilterColumn } from "../src/grid/filters";

const entities = ["Kamppi", "Kallio", "Töölö", "Pasila"].map((name, i) => ({
  id: `urn:ngsi-ld:Station:${i + 1}`,
  type: "Station",
  name: { type: "Property", value: name },
  bikes: { type: "Property", value: i * 2 },
}));
const config = parseGridConfig({
  source: { kind: "fixture", name: "t" },
  type: "Station",
  columns: [{ attr: "name", label: "Name", pinned: true }, { attr: "bikes", label: "Bikes" }],
  pageSize: 10,
}).config!;
const NUMBERS = { selectRow: "Select row", selectAll: "Select all rows" };

function Host({ onSelected }: { onSelected?: (ids: string[]) => void }) {
  const [selected, setSelected] = useState<string[]>([]);
  return (
    <EntityGrid
      config={config}
      source={fixtureSource(entities)}
      rowNumbers={NUMBERS}
      selected={selected}
      onSelectedChange={(ids) => {
        setSelected(ids);
        onSelected?.(ids);
      }}
      filterRow={false}
      rowSize="small"
      footer={(column) => `foot ${column.label}`}
    />
  );
}

describe("EntityGrid rows (T-4883)", () => {
  it("checks one row, a shift-click range and all", async () => {
    const onSelected = vi.fn();
    const { container } = render(<Host onSelected={onSelected} />);
    await waitFor(() => expect(screen.getByText("Pasila")).toBeInTheDocument());
    // No row numbers since owner 2026-11-01: the checkbox column is only the checkbox.
    expect(container.querySelectorAll("tbody .jc-grid-rownum-n")).toHaveLength(0);
    fireEvent.click(screen.getByRole("checkbox", { name: "Select row: Kamppi" }));
    expect(onSelected).toHaveBeenLastCalledWith(["urn:ngsi-ld:Station:1"]);
    fireEvent.click(screen.getByRole("checkbox", { name: "Select row: Töölö" }), { shiftKey: true });
    expect(onSelected).toHaveBeenLastCalledWith(["urn:ngsi-ld:Station:1", "urn:ngsi-ld:Station:2", "urn:ngsi-ld:Station:3"]);
    expect(container.querySelectorAll('tr[aria-selected="true"]')).toHaveLength(3);
    fireEvent.click(screen.getByRole("checkbox", { name: "Select all rows" }));
    expect(onSelected).toHaveBeenLastCalledWith(entities.map((e) => e.id));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select all rows" }));
    expect(onSelected).toHaveBeenLastCalledWith([]);
  });

  it("opens the row from its expand button, draws no filter row, a footer and the row size", async () => {
    const { container } = render(<Host />);
    await waitFor(() => expect(screen.getByText("Kallio")).toBeInTheDocument());
    expect(container.querySelector(".jc-grid-filter-row")).toBeNull();
    expect(container.querySelector(".jc-grid")?.getAttribute("data-row-size")).toBe("small");
    expect(screen.getByText("foot Bikes")).toBeInTheDocument();
    fireEvent.click(container.querySelectorAll<HTMLButtonElement>("tbody .jc-grid-expand")[1]);
    await waitFor(() => expect(screen.getByRole("dialog")).toBeInTheDocument());
  });

  it("ticks a row from the label around its box and opens nothing (T-5596)", async () => {
    const onSelected = vi.fn();
    const { container } = render(<Host onSelected={onSelected} />);
    await waitFor(() => expect(screen.getByText("Kallio")).toBeInTheDocument());
    const box = screen.getByRole("checkbox", { name: "Select row: Kallio" });
    const label = box.closest("label");
    expect(label?.className).toBe("jc-grid-check");
    expect(screen.getByRole("checkbox", { name: "Select all rows" }).closest("label")?.className).toBe("jc-grid-check");
    // A finger lands on the label beside the box: the box ticks and the row stays closed.
    fireEvent.click(label!);
    expect(onSelected).toHaveBeenLastCalledWith(["urn:ngsi-ld:Station:2"]);
    expect(container.querySelector('tr[aria-selected="true"]')).not.toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("gives the label a finger's 44 px under a coarse pointer and no box under a mouse (T-5596)", () => {
    const css = readFileSync(join(__dirname, "../src/grid/grid.css"), "utf8");
    // jsdom applies no media query: the stylesheet is read as the browser would match it.
    const coarse = [...css.matchAll(/@media \(pointer: coarse\) \{([\s\S]*?)\n\}/g)].map((m) => m[1]).join("\n");
    const rule = /\.jc-grid-check \{([^}]*)\}/.exec(coarse)?.[1] ?? "";
    expect(rule).toMatch(/min-width: 44px/);
    expect(rule).toMatch(/min-height: 44px/);
    expect(coarse).toMatch(/\.jc-grid-expand \{\s*display: none;/);
    const fine = css.replace(/@media[^{]*\{[\s\S]*?\n\}/g, "");
    expect(/\.jc-grid-check \{([^}]*)\}/.exec(fine)?.[1]).toMatch(/display: contents/);
  });

  it("hands its filterable columns to the host", async () => {
    const seen: FilterColumn[][] = [];
    render(<EntityGrid config={config} source={fixtureSource(entities)} onFilterColumns={(c) => seen.push(c)} />);
    // The host hears the columns from an effect after the rows render (T-5359): wait for the settled list,
    // not for the row text, which can show before that effect ran.
    await waitFor(() => expect(seen.at(-1)?.find((c) => c.attr === "bikes")?.kind).toBe("number"));
    expect(screen.getByText("Kamppi")).toBeInTheDocument();
    expect(seen.at(-1)?.find((c) => c.attr === "name")?.kind).toBe("text");
  });
});

describe("EntityGrid filter columns after an empty answer (T-5600)", () => {
  it("keeps the kinds of the last rows and marks a LanguageProperty", async () => {
    const named = entities.map((e) => ({ ...e, name: { type: "LanguageProperty", languageMap: { fi: e.name.value } } }));
    const seen: FilterColumn[][] = [];
    const { rerender } = render(<EntityGrid config={config} source={fixtureSource(named)} onFilterColumns={(c) => seen.push(c)} />);
    await waitFor(() => expect(seen.at(-1)?.find((c) => c.attr === "bikes")?.kind).toBe("number"));
    expect(seen.at(-1)?.find((c) => c.attr === "name")?.language).toBe(true);
    // A search that matches nothing: the page is empty, and bikes is still a number, not text a search would ask.
    rerender(<EntityGrid config={config} source={{ ...fixtureSource(named), query: async () => ({ rows: [], total: 0 }) }} onFilterColumns={(c) => seen.push(c)} />);
    await waitFor(() => expect(screen.queryByText("Kamppi")).toBeNull());
    expect(seen.at(-1)?.find((c) => c.attr === "bikes")?.kind).toBe("number");
    expect(seen.at(-1)?.find((c) => c.attr === "name")?.language).toBe(true);
  });
});

describe("EntityGrid reloadKey (T-5186)", () => {
  it("reads the rows again on a new key and keeps the checked rows", async () => {
    const source = fixtureSource(entities);
    const query = vi.spyOn(source, "query");
    const grid = (key: number) => (
      <EntityGrid config={config} source={source} rowNumbers={NUMBERS} selected={["urn:ngsi-ld:Station:2"]} filterRow={false} reloadKey={key} />
    );
    const { rerender, container } = render(grid(0));
    await waitFor(() => expect(screen.getByText("Pasila")).toBeInTheDocument());
    const first = query.mock.calls.length;
    rerender(grid(0));
    expect(query.mock.calls.length).toBe(first);
    rerender(grid(1));
    // The rows and their count, two reads of the one question (T-6541).
    await waitFor(() => expect(query.mock.calls.length).toBe(first + 2));
    expect(container.querySelectorAll('tr[aria-selected="true"]')).toHaveLength(1);
  });
});
