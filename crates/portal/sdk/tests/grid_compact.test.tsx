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
