/**
 * The grid on a phone (T-6748): the row's label leads and opens the row, the id and a Type every row
 * shares leave the table, and the type and its choice show on the line above it.
 */
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";

const entities = ["Kamppi", "Kallio"].map((title, i) => ({
  id: `urn:ngsi-ld:PointOfInterest:00313271${i}`,
  type: "PointOfInterest",
  name: { type: "Property", value: `poi-${i}` },
  title: { type: "Property", value: title },
  category: { type: "Property", value: i === 0 ? "school" : "library" },
}));
const config = parseGridConfig({
  source: { kind: "fixture", name: "t" },
  type: "PointOfInterest",
  columns: [{ attr: "name" }, { attr: "title", label: "Title", pinned: true }, { attr: "category", label: "Category" }],
  pageSize: 10,
}).config!;
const choice = { value: "PointOfInterest", label: "Type", options: [{ value: "PointOfInterest", label: "Point of interest" }, { value: "Station", label: "Station" }] };

const access = ["name", "title", "category"].map((name) => ({ name, read: true, write: false, delete: false, reason: "grant" as const }));

function heads(container: HTMLElement): string[] {
  return [...container.querySelectorAll("thead tr:first-child th")].map((th) => th.textContent ?? "");
}

describe("EntityGrid compact (T-6748)", () => {
  it("moves a value every row shares out of the columns, to the line above", async () => {
    const same = entities.map((each) => ({ ...each, category: { type: "Property", value: "socialService" } }));
    const { container } = render(<EntityGrid config={config} source={fixtureSource(same)} compact filterRow={false} />);
    await waitFor(() => expect(screen.getByText("Kallio")).toBeInTheDocument());
    await waitFor(() => expect(heads(container).some((name) => /^Category/.test(name))).toBe(false));
    const line = container.querySelector(".jc-grid-typeline")!;
    expect(line).toHaveTextContent("Category");
    expect(line).toHaveTextContent("socialService");
    expect(line).toHaveTextContent("PointOfInterest");
    // The label is never moved, even where two rows share it.
    expect(heads(container)[0]).toMatch(/^Title/);
  });

  it("drops a field empty on every row, without a word on the line above", async () => {
    const blank = entities.map((each) => ({ ...each, category: { type: "Property", value: "" } }));
    const { container } = render(<EntityGrid config={config} source={fixtureSource(blank)} compact filterRow={false} />);
    await waitFor(() => expect(screen.getByText("Kallio")).toBeInTheDocument());
    await waitFor(() => expect(heads(container).some((name) => /^Category/.test(name))).toBe(false));
    expect(container.querySelector(".jc-grid-typeline")).not.toHaveTextContent("Category");
  });

  it("offers Try again under a failed read, and the press reads the rows again (T-6781)", async () => {
    const real = fixtureSource(entities);
    let calls = 0;
    const flaky = { ...real, query: (q: Parameters<typeof real.query>[0], page: Parameters<typeof real.query>[1]) => (page.limit > 0 && ++calls === 1 ? Promise.reject(new Error("timeout")) : real.query(q, page)) };
    render(<EntityGrid config={config} source={flaky} compact filterRow={false} />);
    const retry = await screen.findByRole("button", { name: "Try again" });
    fireEvent.click(retry);
    await waitFor(() => expect(screen.getByText("Kallio")).toBeInTheDocument());
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("keeps a column whose rows differ", async () => {
    const { container } = render(<EntityGrid config={config} source={fixtureSource(entities)} compact filterRow={false} />);
    await waitFor(() => expect(screen.getByText("Kallio")).toBeInTheDocument());
    expect(heads(container).some((name) => /^Category/.test(name))).toBe(true);
    expect(container.querySelector(".jc-grid-typeline")).not.toHaveTextContent("library");
  });

  it("leads with the label, drops the id and the shared type, and shows the type above", async () => {
    const onChange = vi.fn();
    const onOpenRow = vi.fn();
    const { container } = render(
      <EntityGrid config={config} source={fixtureSource(entities)} compact filterRow={false} typeChoice={{ ...choice, onChange }} onOpenRow={onOpenRow} access={access} />,
    );
    await waitFor(() => expect(screen.getByText("Kallio")).toBeInTheDocument());
    const names = heads(container);
    expect(names[0]).toMatch(/^Title/);
    expect(names.some((name) => /^ID|^Type/.test(name))).toBe(false);
    expect(container.querySelector("tbody")?.textContent).not.toContain("00313271");
    // No cell carries an inspector "i" on a phone (T-6781): it covered the text in every cell.
    expect(container.querySelectorAll("tbody .jc-grid-info")).toHaveLength(0);
    expect(screen.queryAllByRole("button", { name: /^Your access:/ })).toHaveLength(0);
    // What the person may do with a field stays in reach: the column's menu opens the same panel.
    fireEvent.click(screen.getByRole("button", { name: "Column options: Category" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Your access" }));
    expect(await screen.findByRole("heading", { name: /Category/ })).toBeInTheDocument();
    // The type left the columns, and the choice of another stays on the line above.
    const select = screen.getByRole("combobox", { name: "Type" });
    expect(container.querySelector(".jc-grid-typeline")).toContainElement(select);
    fireEvent.change(select, { target: { value: "Station" } });
    expect(onChange).toHaveBeenCalledWith("Station");
    // The label is the row's way in.
    fireEvent.click(screen.getByRole("button", { name: /Kamppi/ }));
    expect(onOpenRow).toHaveBeenCalledWith("urn:ngsi-ld:PointOfInterest:003132710");
  });

  it("keeps the id, the type column and no type line on a wide screen", async () => {
    const { container } = render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await waitFor(() => expect(screen.getByText("Kallio")).toBeInTheDocument());
    const names = heads(container);
    expect(names[0]).toMatch(/^ID/);
    expect(names[1]).toMatch(/^Type/);
    expect(container.querySelector(".jc-grid-typeline")).toBeNull();
  });

  it("keeps the cell's \"i\" and no access item in the column menu on a wide screen (T-6781)", async () => {
    render(<EntityGrid config={config} source={fixtureSource(entities)} access={access} />);
    await waitFor(() => expect(screen.getByText("Kallio")).toBeInTheDocument());
    expect(screen.getAllByRole("button", { name: "Your access: Category" }).length).toBeGreaterThan(0);
    fireEvent.click(screen.getByRole("button", { name: "Column options: Category" }));
    expect(screen.queryByRole("menuitem", { name: "Your access" })).toBeNull();
  });
});
