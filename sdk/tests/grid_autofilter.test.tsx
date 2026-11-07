import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import type { EntitySource, GridQuery } from "../src/grid/source";
import { columnValues, VALUE_PAGE } from "../src/grid/columnValues";
import { AutoFilter, chipText, FilterChips, HeaderFilter, valueText } from "../src/grid/AutoFilter";
import { DEFAULT_LABELS } from "../src/grid/useEntityGrid";
import type { FilterColumn } from "../src/grid/filters";

// Five KPI rows over two pages: the value list reads the whole answer, not the page (T-6328).
const kpi = (n: number, period: string, value: number) => ({
  id: `urn:ngsi-ld:KeyPerformanceIndicator:dopravne-nehody-s-usmrtenim.pocet.101.${period}.${n}`,
  type: "KeyPerformanceIndicator",
  name: { type: "Property", value: `Nehody ${n}` },
  refPeriod: { type: "Property", value: period },
  kpiValue: { type: "Property", value },
});
const entities = [kpi(1, "2012", 4), kpi(2, "2013", 7), kpi(3, "2012", 4), kpi(4, "2014", 9), kpi(5, "2013", 7)];
const config = parseGridConfig({
  source: { kind: "fixture", name: "t" },
  type: "KeyPerformanceIndicator",
  pageSize: 2,
  columns: [{ attr: "name", label: "Name" }, { attr: "refPeriod", label: "Period" }, { attr: "kpiValue", label: "Value" }],
}).config!;

function recording(rows: Record<string, unknown>[]) {
  const asked: { q?: string; attrs?: string[]; limit: number }[] = [];
  const inner = fixtureSource(rows);
  const source: EntitySource = {
    query: async (q: GridQuery, page) => {
      asked.push({ q: q.q, attrs: q.attrs, limit: page.limit });
      return inner.query(q, page);
    },
    get: inner.get,
  };
  return { asked, source };
}

const openFilter = (label: string) => fireEvent.click(screen.getByRole("button", { name: new RegExp(`^Filter column: ${label}`) }));

describe("the header filter (T-6328)", () => {
  it("lists the column's values over the whole answer, searchable, with Select all", async () => {
    const { asked, source } = recording(entities);
    render(<EntityGrid config={config} source={source} filterRow={false} />);
    await screen.findByText("Nehody 1");
    openFilter("Period");
    const dialog = screen.getByRole("dialog", { name: "Filter: Period" });
    const list = await within(dialog).findByRole("list", { name: "Values" });
    // 2012 and 2013 twice, 2014 once: every row read, though the page holds two.
    expect(within(list).getAllByRole("checkbox").map((box) => box.closest("label")!.textContent)).toEqual(["20122", "20132", "20141"]);
    expect(asked.some((each) => each.attrs?.join() === "refPeriod" && each.limit === VALUE_PAGE)).toBe(true);

    fireEvent.change(within(dialog).getByRole("searchbox"), { target: { value: "13" } });
    expect(within(list).getAllByRole("checkbox")).toHaveLength(1);
    // Select all works on what the search shows.
    const all = within(dialog).getByRole("checkbox", { name: "Select all" });
    expect(all).toBeChecked();
    fireEvent.click(all);
    expect(within(list).getByRole("checkbox")).not.toBeChecked();
    expect(within(dialog).getByRole("button", { name: "Apply" })).toBeEnabled();
    // Nothing ticked asks for nothing a person could mean: Apply waits for a value.
    fireEvent.change(within(dialog).getByRole("searchbox"), { target: { value: "" } });
    expect(all).not.toBeChecked();
    expect((all as HTMLInputElement).indeterminate).toBe(true);
    fireEvent.click(all);
    fireEvent.click(all);
    expect(within(list).getAllByRole("checkbox").every((box) => !(box as HTMLInputElement).checked)).toBe(true);
    expect(within(dialog).getByRole("button", { name: "Apply" })).toBeDisabled();
  });

  it("applies the ticked values as a q, marks the header and shows a chip that one click removes", async () => {
    const { asked, source } = recording(entities);
    render(<EntityGrid config={config} source={source} filterRow={false} />);
    await screen.findByText("Nehody 1");
    openFilter("Period");
    const dialog = screen.getByRole("dialog", { name: "Filter: Period" });
    const list = await within(dialog).findByRole("list", { name: "Values" });
    fireEvent.click(within(list).getByRole("checkbox", { name: /2013/ }));
    fireEvent.click(within(list).getByRole("checkbox", { name: /2014/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply" }));

    // A period is text: the value goes in quoted, as data.
    await waitFor(() => expect(asked.at(-1)?.q).toBe('refPeriod=="2012"'));
    expect(screen.queryByRole("dialog")).toBeNull();
    const button = screen.getByRole("button", { name: "Filter column: Period (filtered)" });
    expect(button).toHaveAttribute("data-active");
    expect(button.closest("th")).toHaveClass("jc-grid-th--filtered");
    const chips = screen.getByRole("list", { name: "Active filters" });
    expect(within(chips).getByText("Period: 2012")).toBeInTheDocument();

    fireEvent.click(within(chips).getByRole("button", { name: "Remove filter Period: 2012" }));
    await waitFor(() => expect(asked.at(-1)?.q).toBeUndefined());
    expect(screen.queryByRole("list", { name: "Active filters" })).toBeNull();
    expect(screen.getByRole("button", { name: "Filter column: Period" })).not.toHaveAttribute("data-active");
  });

  it("asks a number column's values as numbers, and its range condition by both ends", async () => {
    const { asked, source } = recording(entities);
    render(<EntityGrid config={config} source={source} />);
    await screen.findByText("Nehody 1");
    openFilter("Value");
    let dialog = screen.getByRole("dialog", { name: "Filter: Value" });
    const list = await within(dialog).findByRole("list", { name: "Values" });
    fireEvent.click(within(list).getByRole("checkbox", { name: /^9/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(asked.at(-1)?.q).toBe("kpiValue==4,7"));
    // The filter row under the header says the same.
    expect(screen.getByRole("combobox", { name: "Filter: Value" })).toHaveValue("anyOf");
    expect(screen.getByText("4, 7", { selector: ".jc-grid-filter-ticked" })).toBeInTheDocument();

    openFilter("Value");
    dialog = screen.getByRole("dialog", { name: "Filter: Value" });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Condition: Value" }), { target: { value: "between" } });
    fireEvent.change(within(dialog).getByRole("spinbutton", { name: "Value: Value" }), { target: { value: "5" } });
    fireEvent.change(within(dialog).getByRole("spinbutton", { name: "Upper value: Value" }), { target: { value: "8" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(asked.at(-1)?.q).toBe("kpiValue>=5;kpiValue<=8"));
    expect(screen.getByText("Value: 5 – 8")).toBeInTheDocument();
  });

  it("escapes a typed condition: a quote and a regex character stay data", async () => {
    const { asked, source } = recording(entities);
    render(<EntityGrid config={config} source={source} filterRow={false} />);
    await screen.findByText("Nehody 1");
    openFilter("Name");
    const dialog = screen.getByRole("dialog", { name: "Filter: Name" });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Condition: Name" }), { target: { value: "contains" } });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Value: Name" }), { target: { value: 'a"b.(' } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(asked.at(-1)?.q).toBe('name~="a\\"b\\.\\("'));
  });

  it("Escape and a click outside close it, Escape back to its funnel", async () => {
    const { source } = recording(entities);
    render(<EntityGrid config={config} source={source} filterRow={false} />);
    await screen.findByText("Nehody 1");
    openFilter("Period");
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Filter: Period" }), { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Filter column: Period" }));
    openFilter("Period");
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("clears every filter at once from the chips", async () => {
    const { asked, source } = recording(entities);
    render(<EntityGrid config={config} source={source} filterRow={false} />);
    await screen.findByText("Nehody 1");
    for (const label of ["Name", "Value"]) {
      openFilter(label);
      const dialog = screen.getByRole("dialog", { name: `Filter: ${label}` });
      fireEvent.change(within(dialog).getByRole("combobox", { name: `Condition: ${label}` }), { target: { value: "present" } });
      fireEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    }
    await waitFor(() => expect(asked.at(-1)?.q).toBe("name;kpiValue"));
    fireEvent.click(screen.getByRole("button", { name: "Clear all filters" }));
    await waitFor(() => expect(asked.at(-1)?.q).toBeUndefined());
  });
});

describe("the header filter's parts", () => {
  const period: FilterColumn = { key: "refPeriod", attr: "refPeriod", kind: "text" };
  const status: FilterColumn = { key: "status", attr: "status", kind: "enum", options: [{ value: "open", title: "Open" }] };

  it("Only filters to one value at once: the second of two clicks", async () => {
    let applied: unknown;
    const load = async () => ({ values: [{ value: "2012", count: 2 }, { value: "2013", count: 1 }], scanned: 3, total: 3, capped: false });
    render(<AutoFilter column={period} label="Period" labels={DEFAULT_LABELS} filter={undefined} load={load} formatValue={(v) => v} onApply={(f) => (applied = f)} onClose={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Only: 2013" }));
    expect(applied).toEqual({ op: "anyOf", value: "", values: ["2013"] });
  });

  it("says when the list is a sample of the answer", async () => {
    const load = async () => ({ values: [{ value: "2012", count: 5000 }], scanned: 5000, total: 12000, capped: true });
    render(<AutoFilter column={period} label="Period" labels={DEFAULT_LABELS} filter={undefined} load={load} formatValue={(v) => v} onApply={() => {}} onClose={() => {}} />);
    expect(await screen.findByText("Values of the first 5000 of 12000 entities; a condition asks all of them.")).toBeInTheDocument();
  });

  it("the funnel opens and closes, and names a filtered column so", () => {
    const calls: string[] = [];
    const { rerender } = render(
      <HeaderFilter column={period} label="Period" labels={DEFAULT_LABELS} locale="en" filter={undefined} open={false} onOpen={() => calls.push("open")} onClose={() => calls.push("close")} load={async () => ({ values: [], scanned: 0, capped: false })} onApply={() => {}} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Filter column: Period" }));
    rerender(
      <HeaderFilter column={period} label="Period" labels={DEFAULT_LABELS} locale="en" filter={{ op: "present", value: "" }} open onOpen={() => calls.push("open")} onClose={() => calls.push("close")} load={async () => ({ values: [], scanned: 0, capped: false })} onApply={() => {}} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Filter column: Period (filtered)" }));
    expect(calls).toEqual(["open", "close"]);
  });

  it("chips read each filter in the column's own words and remove one or all", () => {
    expect(valueText(status, "open", "en")).toBe("Open");
    expect(valueText({ key: "n", attr: "n", kind: "number" }, "1234.5", "sk")).toBe(new Intl.NumberFormat("sk").format(1234.5));
    expect(chipText(period, "Period", { op: "anyOf", value: "", values: ["1", "2", "3", "4", "5"] }, DEFAULT_LABELS, "en")).toBe("Period: 1, 2, 3 +2");
    expect(chipText(period, "Period", { op: "empty", value: "" }, DEFAULT_LABELS, "en")).toBe("Period is empty");
    const removed: unknown[] = [];
    render(
      <FilterChips
        columns={[{ key: "refPeriod", label: "Period" }, { key: "status", label: "Status" }]}
        filterable={new Map([["refPeriod", period], ["status", status]])}
        filters={{ refPeriod: { op: "contains", value: "20" }, status: { op: "anyOf", value: "", values: ["open"] } }}
        labels={DEFAULT_LABELS}
        locale="en"
        onRemove={(keys) => removed.push(keys)}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove filter Status: Open" }));
    fireEvent.click(screen.getByRole("button", { name: "Clear all filters" }));
    expect(removed).toEqual(["status", ["refPeriod", "status"]]);
  });
});

describe("columnValues", () => {
  it("says when it read less than the answer holds", async () => {
    const pages: number[] = [];
    const source: EntitySource = {
      query: async (_q, page) => {
        pages.push(page.offset);
        const rows = Array.from({ length: VALUE_PAGE }, (_, i) => ({ id: `u${page.offset + i}`, type: "T", cells: { a: { kind: "property" as const, value: (page.offset + i) % 3 } }, raw: {} }));
        return { rows, total: 12_000 };
      },
      get: async () => null,
    };
    const read = await columnValues(source, { type: "T" }, "a");
    expect(read.capped).toBe(true);
    expect(read.scanned).toBe(5000);
    expect(pages).toEqual([0, 1000, 2000, 3000, 4000]);
    expect(read.values.map((each) => each.value)).toEqual(["0", "1", "2"]);
  });
});
