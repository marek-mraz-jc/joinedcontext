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
    // The label is the row's way in alone: no inspector "i" beside the name.
    expect(container.querySelectorAll("tbody td.jc-grid-pinned .jc-grid-info")).toHaveLength(0);
    // The other cells keep theirs: what the person may do with a value stays one tap away.
    expect(screen.getAllByRole("button", { name: "Your access: Category" }).length).toBeGreaterThan(0);
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
});
