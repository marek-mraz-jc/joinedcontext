import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import type { ColumnLayout } from "../src/grid/useEntityGrid";

const entities = [
  { id: "urn:ngsi-ld:Stop:a", type: "Stop", name: { type: "Property", value: "Alpha" }, count: { type: "Property", value: 2, observedAt: "2026-11-10T08:00:00Z" }, "ngsi-ld:status": { type: "Property", value: "open" } },
  { id: "urn:ngsi-ld:Stop:b", type: "Stop", name: { type: "Property", value: "Beta" }, count: { type: "Property", value: 9 }, "ngsi-ld:status": { type: "Property", value: "shut" } },
];
const config = parseGridConfig({
  source: { kind: "fixture", name: "t" },
  type: "Stop",
  columns: [{ attr: "name", label: "Name", pinned: true }, { attr: "count", label: "Count" }, { attr: "ngsi-ld:status" }],
}).config!;

/** The attribute columns' labels in header order (the id, the type and the timestamps carry no data-col-attr). */
const headers = () => [...document.querySelectorAll("th[data-col-attr] .jc-grid-sort-btn")].map((b) => b.textContent?.replace(/ [↑↓]$/, ""));

/** A host that keeps the layout, as the Portal keeps it in the viewer's saved view. */
function Kept({ onLayout }: { onLayout?: (next: ColumnLayout) => void }) {
  const [layout, setLayout] = useState<ColumnLayout>({});
  return (
    <EntityGrid
      config={config}
      source={fixtureSource(entities)}
      layout={layout}
      onLayout={(next) => {
        setLayout(next);
        onLayout?.(next);
      }}
    />
  );
}

const openPanel = () => {
  fireEvent.click(screen.getByRole("button", { name: "Arrange columns" }));
  return screen.getByRole("dialog", { name: "Arrange columns" });
};

describe("the toolbar's Arrange columns panel (T-6738)", () => {
  it("opens from the toolbar with a hint, and Escape closes it back to its button", async () => {
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    const toggle = screen.getByRole("button", { name: "Arrange columns" });
    expect(toggle.closest(".jc-grid-toolbar")).not.toBeNull();
    const panel = openPanel();
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(panel).toHaveAccessibleDescription(/Only your view changes, never the data/);
    expect(document.activeElement).toBe(within(panel).getByRole("heading", { name: "Arrange columns" }));
    fireEvent.keyDown(panel, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Arrange columns" })).toBeNull();
    expect(document.activeElement).toBe(toggle);
  });

  it("hides and shows a column again; the pinned name cannot be hidden", async () => {
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    const panel = openPanel();
    expect(within(panel).getByRole("checkbox", { name: "Show field: Name" })).toBeDisabled();
    fireEvent.click(within(panel).getByRole("checkbox", { name: "Show field: Count" }));
    await waitFor(() => expect(headers()).toEqual(["Name", "status"]));
    const back = within(screen.getByRole("dialog", { name: "Arrange columns" })).getByRole("checkbox", { name: "Show field: Count" });
    expect(back).not.toBeChecked();
    fireEvent.click(back);
    await waitFor(() => expect(headers()).toEqual(["Name", "Count", "status"]));
  });

  it("hands a hide to the host that holds hidden", async () => {
    const onHide = vi.fn();
    render(<EntityGrid config={config} source={fixtureSource(entities)} onHide={onHide} />);
    await screen.findByText("Alpha");
    fireEvent.click(within(openPanel()).getByRole("checkbox", { name: "Show field: status" }));
    expect(onHide).toHaveBeenCalledWith("ngsi-ld:status");
  });

  it("orders, renames and resets in the viewer's layout, never past the name", async () => {
    const onLayout = vi.fn();
    render(<Kept onLayout={onLayout} />);
    await screen.findByText("Alpha");
    let panel = openPanel();
    expect(within(panel).getByRole("button", { name: "Move up: Count" })).toBeDisabled();
    expect(within(panel).getByRole("button", { name: "Move up: Name" })).toBeDisabled();
    fireEvent.click(within(panel).getByRole("button", { name: "Move down: Count" }));
    expect(onLayout).toHaveBeenLastCalledWith({ order: ["name", "ngsi-ld:status", "count"] });
    await waitFor(() => expect(headers()).toEqual(["Name", "status", "Count"]));
    panel = screen.getByRole("dialog", { name: "Arrange columns" });
    const field = within(panel).getByRole("textbox", { name: "Label in this view: status" });
    expect(field).toHaveAttribute("placeholder", "status");
    fireEvent.change(field, { target: { value: "  State  " } });
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onLayout).toHaveBeenLastCalledWith({ order: ["name", "ngsi-ld:status", "count"], labels: { "ngsi-ld:status": "State" } });
    await waitFor(() => expect(headers()).toEqual(["Name", "State", "Count"]));
    // Emptied, the label goes back to the model's.
    const again = within(screen.getByRole("dialog", { name: "Arrange columns" })).getByRole("textbox", { name: "Label in this view: status" });
    fireEvent.change(again, { target: { value: "" } });
    fireEvent.blur(again);
    await waitFor(() => expect(headers()).toEqual(["Name", "status", "Count"]));
    fireEvent.click(within(screen.getByRole("dialog", { name: "Arrange columns" })).getByRole("button", { name: "Reset columns" }));
    expect(onLayout).toHaveBeenLastCalledWith({});
    await waitFor(() => expect(headers()).toEqual(["Name", "Count", "status"]));
  });

  it("keeps a typed label as text, never as markup", async () => {
    render(<Kept />);
    await screen.findByText("Alpha");
    const field = within(openPanel()).getByRole("textbox", { name: "Label in this view: Count" });
    fireEvent.change(field, { target: { value: "<img src=x onerror=alert(1)>" } });
    fireEvent.keyDown(field, { key: "Enter" });
    await waitFor(() => expect(headers()).toContain("<img src=x onerror=alert(1)>"));
    expect(document.querySelector("thead img")).toBeNull();
  });

  it("offers the metadata columns under a plain heading, each with its hint, off by default", async () => {
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    const panel = openPanel();
    const extra = within(panel).getByRole("group", { name: "Extra details" });
    within(extra).getByText("Show when the value was measured");
    within(extra).getByText("Show the unit the value is given in");
    for (const box of within(extra).getAllByRole("checkbox")) expect(box).not.toBeChecked();
    fireEvent.change(within(extra).getByRole("combobox", { name: "Details for" }), { target: { value: "count" } });
    fireEvent.click(within(extra).getByRole("checkbox", { name: /^Observed/ }));
    await waitFor(() => expect(screen.getAllByRole("columnheader").some((th) => /Observed/.test(th.textContent ?? ""))).toBe(true));
    expect(within(extra).getByRole("checkbox", { name: /^Observed/ })).toBeChecked();
  });
});
