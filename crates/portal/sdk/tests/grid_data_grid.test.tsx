import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { DataGrid, DEFAULT_DATA_LABELS } from "../src/grid/DataGrid";
import { arrangeRows, EMPTY_QUERY, passes, sortDataRows, valuesOf } from "../src/grid/arrayRows";
import type { DataColumn } from "../src/grid/arrayRows";

interface Policy {
  name: string;
  space: string;
  grants: number;
  changed: string;
}

const rows: Policy[] = [
  { name: "Readers", space: "air", grants: 2, changed: "2026-11-02T10:00:00Z" },
  { name: "Admins", space: "", grants: 10, changed: "2026-11-09T10:00:00Z" },
  { name: "buses", space: "transport", grants: 1, changed: "" },
];
const columns: DataColumn<Policy>[] = [
  { key: "name", label: "Name", text: (p) => p.name },
  { key: "space", label: "Space", text: (p) => p.space },
  { key: "grants", label: "Grants", text: (p) => String(p.grants), kind: "number" },
  { key: "changed", label: "Changed", text: (p) => p.changed, kind: "date", hidden: true },
];
const names = (list: Policy[]) => list.map((p) => p.name);

describe("array row source", () => {
  it("sorts text case-blind, numbers as numbers, empty values last both ways", () => {
    expect(names(sortDataRows(rows, columns, { key: "name", dir: "asc" }))).toEqual(["Admins", "buses", "Readers"]);
    expect(names(sortDataRows(rows, columns, { key: "grants", dir: "desc" }))).toEqual(["Admins", "Readers", "buses"]);
    expect(names(sortDataRows(rows, columns, { key: "space", dir: "asc" }))).toEqual(["Readers", "buses", "Admins"]);
    expect(names(sortDataRows(rows, columns, { key: "space", dir: "desc" }))).toEqual(["buses", "Readers", "Admins"]);
    expect(names(sortDataRows(rows, columns, { key: "changed", dir: "asc" }))).toEqual(["Readers", "Admins", "buses"]);
  });

  it("filters by condition and by picked values, and searches every column", () => {
    const q = { ...EMPTY_QUERY };
    expect(names(arrangeRows(rows, columns, { ...q, filters: { grants: { op: "gt", value: "1" } } }).rows)).toEqual(["Readers", "Admins"]);
    expect(names(arrangeRows(rows, columns, { ...q, filters: { grants: { op: "between", value: "1", value2: "2" } } }).rows)).toEqual(["Readers", "buses"]);
    expect(names(arrangeRows(rows, columns, { ...q, filters: { space: { op: "empty", value: "" } } }).rows)).toEqual(["Admins"]);
    expect(names(arrangeRows(rows, columns, { ...q, filters: { space: { op: "anyOf", value: "", values: ["air", "transport"] } } }).rows)).toEqual(["Readers", "buses"]);
    expect(names(arrangeRows(rows, columns, { ...q, search: "TRANS" }).rows)).toEqual(["buses"]);
    // An unfinished condition narrows nothing; an unknown one lets nothing through.
    expect(passes("x", "text", { op: "contains", value: "" })).toBe(true);
    expect(passes("x", "text", { op: "pattern", value: "x" })).toBe(false);
  });

  it("pages, clamps an offset past the end and counts what the query keeps", () => {
    const many = Array.from({ length: 23 }, (_, i) => ({ name: `p${String(i).padStart(2, "0")}`, space: "s", grants: i, changed: "" }));
    const last = arrangeRows(many, columns, { ...EMPTY_QUERY, pageSize: 10, offset: 20 });
    expect(last.total).toBe(23);
    expect(names(last.rows)).toEqual(["p20", "p21", "p22"]);
    expect(names(arrangeRows(many, columns, { ...EMPTY_QUERY, pageSize: 10, offset: 90 }).rows)).toEqual(["p20", "p21", "p22"]);
    expect(arrangeRows([], columns, EMPTY_QUERY)).toEqual({ rows: [], total: 0 });
  });

  it("tallies a column's values most frequent first", () => {
    expect(valuesOf([...rows, rows[0]!], columns[1]!).values[0]).toEqual({ value: "air", count: 2 });
  });
});

describe("DataGrid", () => {
  const grid = (props: Partial<Parameters<typeof DataGrid<Policy>>[0]> = {}) =>
    render(<DataGrid caption="Policies" rows={rows} columns={columns} rowKey={(p) => p.name} empty={<p>No policies yet. Add one.</p>} {...props} />);
  const bodyNames = () => [...document.querySelectorAll("tbody tr")].map((tr) => tr.firstElementChild?.textContent);

  it("says what to do when the list is empty, and shows a skeleton while it loads", () => {
    const { rerender } = grid({ rows: [] });
    expect(screen.getByText("No policies yet. Add one.")).toBeTruthy();
    // Nothing to page through: no pager.
    expect(screen.queryByRole("button", { name: /Next/ })).toBeNull();
    rerender(<DataGrid caption="Policies" rows={[]} columns={columns} rowKey={(p) => p.name} empty={<p>none</p>} loading />);
    expect(screen.getByRole("table").getAttribute("aria-busy")).toBe("true");
    expect(screen.getByRole("status").textContent).toBe("Loading…");
  });

  it("shows the host's failure panel instead of the table", () => {
    grid({ error: <p role="alert">The server refused: 503</p> });
    expect(screen.getByRole("alert").textContent).toContain("503");
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("sorts from the header menu in the column's own words, and clears a search that hides every row", () => {
    grid();
    fireEvent.click(screen.getByRole("button", { name: "Column options: Grants" }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Largest first" }));
    expect(bodyNames()).toEqual(["Admins", "Readers", "buses"]);
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "nothing like this" } });
    expect(screen.getByText("No rows match the search or the filters.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Clear all filters" }));
    expect(bodyNames()).toHaveLength(3);
  });

  it("filters by ticked values from the header menu", async () => {
    grid();
    fireEvent.click(screen.getByRole("button", { name: "Column options: Space" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Filter…" }));
    const dialog = screen.getByRole("dialog", { name: "Filter: Space" });
    fireEvent.click(await within(dialog).findByRole("button", { name: "Only: transport" }));
    expect(bodyNames()).toEqual(["buses"]);
    expect(document.querySelector("th.jc-grid-th--filtered")?.textContent).toContain("Space");
  });

  it("hides, shows and renames columns in Arrange columns, kept in this browser", () => {
    const { unmount } = grid({ storageKey: "test-policies" });
    expect(screen.queryByRole("button", { name: /Column options: Changed/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Arrange columns" }));
    const panel = screen.getByRole("dialog", { name: "Arrange columns" });
    expect((within(panel).getByRole("checkbox", { name: "Show field: Name" }) as HTMLInputElement).disabled).toBe(true);
    fireEvent.click(within(panel).getByRole("checkbox", { name: "Show field: Changed" }));
    fireEvent.click(within(panel).getByRole("checkbox", { name: "Show field: Space" }));
    const rename = within(panel).getByRole("textbox", { name: "Label in this view: Grants" });
    fireEvent.change(rename, { target: { value: "Rules" } });
    fireEvent.blur(rename);
    unmount();
    grid({ storageKey: "test-policies" });
    expect(screen.getByRole("button", { name: "Column options: Changed" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Column options: Space" })).toBeNull();
    expect(screen.getByRole("button", { name: "Column options: Rules" })).toBeTruthy();
  });

  it("keeps a row's actions column always shown", () => {
    const withActions: DataColumn<Policy>[] = [...columns, { key: "actions", label: "Actions", text: () => "", actions: true, cell: (p) => <button type="button">Open {p.name}</button> }];
    grid({ columns: withActions });
    expect(screen.queryByRole("button", { name: "Column options: Actions" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Arrange columns" }));
    expect((within(screen.getByRole("dialog", { name: "Arrange columns" })).getByRole("checkbox", { name: "Show field: Actions" }) as HTMLInputElement).disabled).toBe(true);
    // Its empty text is no search target and no phone-card label.
    expect(document.querySelector('td[data-label="Actions"]')).toBeNull();
  });

  it("pages through a long list", () => {
    const many = Array.from({ length: 30 }, (_, i) => ({ name: `p${String(i).padStart(2, "0")}`, space: "s", grants: i, changed: "" }));
    grid({ rows: many });
    expect(bodyNames()).toHaveLength(25);
    expect(screen.getByText("30 rows")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Page 2" }));
    expect(bodyNames()).toEqual(["p25", "p26", "p27", "p28", "p29"]);
  });

  it("leaves the pager out for one fixed page size and one page, the host paging the list itself", () => {
    grid({ rows, pageSizes: [50] });
    expect(bodyNames()).toHaveLength(3);
    expect(screen.queryByText("3 rows")).toBeNull();
    expect(screen.queryByRole("button", { name: /Next/ })).toBeNull();
  });

  it("counts a short list without a pager to step through", () => {
    grid();
    expect(screen.getByText("3 rows")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Next/ })).toBeNull();
    expect(screen.queryByLabelText(DEFAULT_DATA_LABELS.rowsPerPage)).toBeNull();
  });

  it("in server mode shows the host's rows as given and reports the query", () => {
    const onQueryChange = vi.fn();
    grid({ query: EMPTY_QUERY, onQueryChange, total: 99 });
    expect(bodyNames()).toEqual(["Readers", "Admins", "buses"]);
    fireEvent.click(screen.getByRole("button", { name: "Name" }));
    expect(onQueryChange).toHaveBeenCalledWith(expect.objectContaining({ sort: { key: "name", dir: "asc" }, offset: 0 }));
    expect(screen.getByText("99 rows")).toBeTruthy();
  });
});
