import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { EntityGrid, moveTo } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import { readableName } from "../src/grid/readableName";
import type { ColumnLayout } from "../src/grid/useEntityGrid";

const entities = [
  { id: "urn:ngsi-ld:Stop:a", type: "Stop", name: { type: "Property", value: "Alpha" }, count: { type: "Property", value: 2 }, "ngsi-ld:status": { type: "Property", value: "open" } },
  { id: "urn:ngsi-ld:Stop:b", type: "Stop", name: { type: "Property", value: "Beta" }, count: { type: "Property", value: 9 }, "ngsi-ld:status": { type: "Property", value: "shut" } },
  { id: "urn:ngsi-ld:Stop:c", type: "Stop", name: { type: "Property", value: "Gamma" }, count: { type: "Property", value: 5 }, "ngsi-ld:status": { type: "Property", value: "open" } },
];
const config = parseGridConfig({
  source: { kind: "fixture", name: "t" },
  type: "Stop",
  columns: [{ attr: "name", label: "Name", pinned: true }, { attr: "count", label: "Count" }, { attr: "ngsi-ld:status" }],
}).config!;

// The id and the type lead every grid (T-6210); these tests read the attribute columns after them.
const headers = () => screen.getAllByRole("columnheader").slice(2, 5).map((th) => within(th).getAllByRole("button")[0].textContent?.replace(/ [↑↓]$/, ""));
const names = () => screen.getAllByRole("button", { name: /^Open: / }).map((b) => b.textContent);
const openMenu = (label: string) => fireEvent.click(screen.getByRole("button", { name: `Column options: ${label}` }));

/** A host that keeps the layout, as the Portal keeps it in a saved view. */
function Kept({ initial, onLayout }: { initial?: ColumnLayout; onLayout?: (next: ColumnLayout) => void }) {
  const [layout, setLayout] = useState<ColumnLayout>(initial ?? {});
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

describe("the column header menu (T-4827)", () => {
  // T-6210 (owner 2026-10-31): id, type, name first and fixed; the type's own attributes; then the
  // entity timestamps; the scope last. The column menu follows the same order.
  it("orders id, type, name, the attributes, createdAt, modifiedAt and the scope last", async () => {
    const scoped = entities.map((entity) => ({ ...entity, scope: "/city/stops", createdAt: "2026-10-01T00:00:00Z", modifiedAt: "2026-10-02T00:00:00Z" }));
    const ordered = parseGridConfig({
      source: { kind: "fixture", name: "scoped" },
      type: "Stop",
      columns: [{ attr: "count", label: "Count" }, { attr: "ngsi-ld:status" }, { attr: "name", label: "Name" }],
      entityTimestamps: true,
    }).config!;
    render(<EntityGrid config={ordered} source={fixtureSource(scoped)} />);
    await screen.findByText("Alpha");
    const all = screen.getAllByRole("columnheader").slice(0, 8).map((th) => within(th).queryAllByRole("button")[0]?.textContent?.replace(/ [↑↓]$/, "") ?? th.textContent);
    expect(all).toEqual(["ID", "Type", "Name", "Count", "status", "Created", "Modified", "Scope"]);
    expect(screen.getAllByText("/city/stops")).toHaveLength(3);
    expect(screen.getAllByText("Stop").length).toBeGreaterThanOrEqual(3);
    // The name is neither hidden nor moved.
    openMenu("Name");
    expect(screen.queryByRole("button", { name: "Hide column" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Move left" })).toBeNull();
  });

  it("names a compact IRI by its local name and shows what each column holds", async () => {
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    expect(headers()).toEqual(["Name", "Count", "status"]);
    expect(readableName("https://smartdatamodels.org/dataModel.Transportation/vocab#routeColor")).toBe("routeColor");
    expect(screen.getAllByRole("columnheader").slice(0, 2).map((th) => th.textContent)).toEqual(["ID", "Type"]);
    const count = screen.getAllByRole("columnheader")[3];
    expect(within(count).getByRole("img", { name: "Number" })).toBeInTheDocument();
  });

  it("sorts ascending and descending", async () => {
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    openMenu("Count");
    fireEvent.click(screen.getByRole("button", { name: "Sort Z → A" }));
    await waitFor(() => expect(names()).toEqual(["b", "c", "a"]));
    openMenu("Count");
    fireEvent.click(screen.getByRole("button", { name: "Sort A → Z" }));
    await waitFor(() => expect(names()).toEqual(["a", "c", "b"]));
  });

  it("Filter… opens the column's header filter, its values searchable (T-6328)", async () => {
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    openMenu("Count");
    fireEvent.click(screen.getByRole("button", { name: "Filter…" }));
    const dialog = screen.getByRole("dialog", { name: "Filter: Count" });
    expect(document.activeElement).toBe(within(dialog).getByRole("searchbox", { name: "Search values: Count" }));
  });

  it("is short: sort, filter and hide, the rest folded under the column's settings (T-6328)", async () => {
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    openMenu("Count");
    const menu = screen.getByRole("dialog", { name: "Column options: Count" });
    const top = [...menu.children].filter((el) => el.tagName === "BUTTON").map((el) => el.textContent);
    expect(top).toEqual(["↑Sort A → Z", "↓Sort Z → A", "Filter…", "⊘Hide column"]);
    const settings = within(menu).getByText("Column settings").closest("details")!;
    expect(settings.open).toBe(false);
    expect(within(settings).getByLabelText("Label in this view")).toBeInTheDocument();
    expect(within(settings).getByRole("button", { name: "Wider" })).toBeInTheDocument();
  });

  it("hides a field, by the host where it holds hidden, else by itself; the name is never hidden", async () => {
    const onHide = vi.fn();
    const { unmount } = render(<EntityGrid config={config} source={fixtureSource(entities)} onHide={onHide} />);
    await screen.findByText("Alpha");
    openMenu("Count");
    fireEvent.click(screen.getByRole("button", { name: "Hide column" }));
    expect(onHide).toHaveBeenCalledWith("count");
    openMenu("Name");
    expect(screen.queryByRole("button", { name: "Hide column" })).toBeNull();
    unmount();
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    openMenu("Count");
    fireEvent.click(screen.getByRole("button", { name: "Hide column" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Column options: Count" })).toBeNull());
  });

  it("names the fields it hid by count, and one click brings each back (T-5620)", async () => {
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    expect(screen.queryByRole("button", { name: /^Hidden fields/ })).toBeNull();
    openMenu("Count");
    fireEvent.click(screen.getByRole("button", { name: "Hide column" }));
    openMenu("status");
    fireEvent.click(screen.getByRole("button", { name: "Hide column" }));
    const toggle = await screen.findByRole("button", { name: "Hidden fields (2)" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    const list = screen.getByRole("list", { name: "Hidden fields" });
    fireEvent.click(within(list).getByRole("button", { name: "Show field: Count" }));
    await screen.findByRole("button", { name: "Column options: Count" });
    expect(screen.getByRole("button", { name: "Hidden fields (1)" })).toBe(document.activeElement);
    expect(screen.queryByRole("button", { name: "Show all" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show field: status" }));
    const status = await screen.findByRole("button", { name: "Column options: status" });
    expect(screen.queryByRole("button", { name: /^Hidden fields/ })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(status));
  });

  it("Show all brings every hidden field back; a host's own hidden fields are not offered", async () => {
    const { unmount } = render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    openMenu("Count");
    fireEvent.click(screen.getByRole("button", { name: "Hide column" }));
    openMenu("status");
    fireEvent.click(screen.getByRole("button", { name: "Hide column" }));
    fireEvent.click(await screen.findByRole("button", { name: "Hidden fields (2)" }));
    fireEvent.click(screen.getByRole("button", { name: "Show all" }));
    await screen.findByRole("button", { name: "Column options: Count" });
    expect(screen.getByRole("button", { name: "Column options: status" })).toBeTruthy();
    unmount();
    // An Endpoint's projection hides `count` for everyone: that is not the person's to undo here.
    render(<EntityGrid config={config} source={fixtureSource(entities)} hidden={["count"]} />);
    await screen.findByText("Alpha");
    expect(screen.queryByRole("button", { name: /^Hidden fields/ })).toBeNull();
  });

  it("renames a column for this view and goes back to the model's label", async () => {
    const onLayout = vi.fn();
    render(<Kept onLayout={onLayout} />);
    await screen.findByText("Alpha");
    openMenu("status");
    const field = screen.getByLabelText("Label in this view");
    fireEvent.change(field, { target: { value: "State" } });
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onLayout).toHaveBeenLastCalledWith({ labels: { "ngsi-ld:status": "State" } });
    await waitFor(() => expect(headers()[2]).toBe("State"));
    fireEvent.click(screen.getByRole("button", { name: "Use the model's label" }));
    await waitFor(() => expect(headers()[2]).toBe("status"));
  });

  it("widens and narrows a column within the store's bounds", async () => {
    const onLayout = vi.fn();
    render(<Kept onLayout={onLayout} initial={{ width: { count: 60 } }} />);
    await screen.findByText("Alpha");
    openMenu("Count");
    fireEvent.click(screen.getByRole("button", { name: "Wider" }));
    expect(onLayout).toHaveBeenLastCalledWith({ width: { count: 100 } });
    fireEvent.click(screen.getByRole("button", { name: "Narrower" }));
    fireEvent.click(screen.getByRole("button", { name: "Narrower" }));
    expect(onLayout).toHaveBeenLastCalledWith({ width: { count: 40 } });
    expect(screen.getAllByRole("columnheader")[3]).toHaveStyle({ width: "40px" });
  });

  it("moves a column by the menu and by dragging its header; nothing moves before the name", async () => {
    const onLayout = vi.fn();
    render(<Kept onLayout={onLayout} />);
    await screen.findByText("Alpha");
    openMenu("Count");
    expect(screen.getByRole("button", { name: "Move left" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Move right" }));
    expect(onLayout).toHaveBeenLastCalledWith({ order: ["name", "ngsi-ld:status", "count"] });
    await waitFor(() => expect(headers()).toEqual(["Name", "status", "Count"]));
    const store = new Map<string, string>();
    const dataTransfer = { setData: (k: string, v: string) => store.set(k, v), getData: (k: string) => store.get(k) ?? "", types: ["application/x-jc-grid-column"], effectAllowed: "" };
    const [, , , status, count] = screen.getAllByRole("columnheader");
    fireEvent.dragStart(count, { dataTransfer });
    fireEvent.dragOver(status, { dataTransfer });
    fireEvent.drop(status, { dataTransfer });
    await waitFor(() => expect(headers()).toEqual(["Name", "Count", "status"]));
    expect(moveTo(["a", "b", "c"], "c", 0)).toEqual(["c", "a", "b"]);
  });

  it("keeps the metadata under the column's settings, and Escape closes the menu back to its button", async () => {
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    openMenu("Count");
    const menu = screen.getByRole("dialog", { name: "Column options: Count" });
    expect(within(menu).getByText("Column settings").tagName).toBe("SUMMARY");
    expect(within(menu).getByLabelText("Observed")).toBeInTheDocument();
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Column options: Count" }));
    // A touch screen has no Escape: a tap outside the menu closes it.
    openMenu("Count");
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  // The grid clips what overflows it: the menu is placed in the window, below its header cell
  // or above it when it does not fit below (T-4924).
  it("places the menu inside the window, above the header when the room below is too short", async () => {
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    const height = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(400);
    const rect = (top: number) => ({ top, bottom: top + 30, left: 0, right: 300, width: 300, height: 30, x: 0, y: top, toJSON: () => ({}) });
    const cell = vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue(rect(70) as DOMRect);
    try {
      openMenu("Count");
      const below = screen.getByRole("dialog", { name: "Column options: Count" });
      expect(below.style.top).toBe("100px");
      expect(below.style.maxHeight).toBe(`${window.innerHeight - 100}px`);
      fireEvent.keyDown(below, { key: "Escape" });

      cell.mockReturnValue(rect(window.innerHeight - 100) as DOMRect);
      openMenu("Count");
      const above = screen.getByRole("dialog", { name: "Column options: Count" });
      expect(above.style.top).toBe(`${window.innerHeight - 100 - 400}px`);
      expect(above.style.maxHeight).toBe(`${window.innerHeight - 100}px`);
    } finally {
      height.mockRestore();
      cell.mockRestore();
    }
  });

  // The first column's popup ran left under the Portal's sidebar, its field cut off (T-6454).
  it("keeps the menu right of the grid's own left edge", async () => {
    const { container } = render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    const width = vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(320);
    const grid = container.querySelector(".jc-grid") as HTMLElement;
    const rect = vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      const left = this === grid ? 180 : 200;
      return { top: 70, bottom: 100, left, right: left + 100, width: 100, height: 30, x: left, y: 70, toJSON: () => ({}) } as DOMRect;
    });
    try {
      openMenu("Count");
      expect(screen.getByRole("dialog", { name: "Column options: Count" }).style.left).toBe("180px");
    } finally {
      width.mockRestore();
      rect.mockRestore();
    }
  });

  // A popup opening upward stayed under the Portal's sticky header, its title covered (T-5181).
  it("keeps the menu below the host's sticky header, up or down", async () => {
    const sticky = document.head.appendChild(document.createElement("style"));
    sticky.textContent = ".jc-grid-col-menu { scroll-margin-top: 56px; }";
    render(<EntityGrid config={config} source={fixtureSource(entities)} />);
    await screen.findByText("Alpha");
    const height = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(900);
    const rect = (top: number) => ({ top, bottom: top + 30, left: 0, right: 300, width: 300, height: 30, x: 0, y: top, toJSON: () => ({}) });
    const cell = vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue(rect(window.innerHeight - 100) as DOMRect);
    try {
      openMenu("Count");
      const above = screen.getByRole("dialog", { name: "Column options: Count" });
      expect(above.style.top).toBe("56px");
      expect(above.style.maxHeight).toBe(`${window.innerHeight - 100 - 56}px`);
      fireEvent.keyDown(above, { key: "Escape" });

      // The header cell scrolled under the sticky header: the menu still starts below it.
      cell.mockReturnValue(rect(-10) as DOMRect);
      openMenu("Count");
      const below = screen.getByRole("dialog", { name: "Column options: Count" });
      expect(below.style.top).toBe("56px");
      expect(below.style.maxHeight).toBe(`${window.innerHeight - 56}px`);
    } finally {
      height.mockRestore();
      cell.mockRestore();
      sticky.remove();
    }
  });

  it("a layout the host kept comes back applied after a remount", async () => {
    const { unmount } = render(<Kept initial={{ order: ["name", "ngsi-ld:status"], labels: { count: "Riders" }, width: { count: 120 } }} />);
    await screen.findByText("Alpha");
    expect(headers()).toEqual(["Name", "status", "Riders"]);
    unmount();
    render(<Kept initial={{ order: ["name", "ngsi-ld:status"], labels: { count: "Riders" }, width: { count: 120 } }} />);
    await screen.findByText("Alpha");
    expect(headers()).toEqual(["Name", "status", "Riders"]);
    expect(screen.getAllByRole("columnheader")[4]).toHaveStyle({ width: "120px" });
  });
});
