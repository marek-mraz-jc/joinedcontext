/**
 * The shared grid by what a person comes to do (T-7063): page through a large set, find nothing and
 * get back, combine filters, sort with gaps, follow a link and return, survive a slow or failing
 * broker, read without any way to write, be refused a write, keep a hidden column, and do it all by
 * keyboard. The phone width and the sticky header are checked in a browser (e2e/grid_ux.spec.ts).
 */
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource, SourceError } from "../src/grid/source";
import type { EntitySource, GridQuery } from "../src/grid/source";
import { DEFAULT_LABELS } from "../src/grid/useEntityGrid";

const p = (value: unknown) => ({ type: "Property", value });

function station(n: number, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { id: `urn:ngsi-ld:Station:hel:${String(n).padStart(5, "0")}`, type: "Station", name: p(`Station ${n}`), ...extra };
}

function configOf(columns: { attr: string; label?: string; pinned?: boolean }[], extra: Record<string, unknown> = {}) {
  const parsed = parseGridConfig({ source: { kind: "fixture", name: "t" }, type: "Station", columns, pageSize: 50, ...extra });
  expect(parsed.findings, JSON.stringify(parsed.findings)).toEqual([]);
  return parsed.config!;
}

/** The rows in the order shown, by the name each row's opener reads. */
const shownNames = () => screen.getAllByRole("button", { name: /^Open: Station/ }).map((b) => b.getAttribute("aria-label")!.slice("Open: ".length));

const NAME_BIKES = [
  { attr: "name", label: "Name" },
  { attr: "bikes", label: "Bikes" },
];

describe("finding the rows (T-7063)", () => {
  it("a visitor pages through 10 000 stations and reaches the last page in one click", async () => {
    const many = Array.from({ length: 10_000 }, (_, i) => station(i));
    render(<EntityGrid config={configOf([{ attr: "name", label: "Name" }])} source={fixtureSource(many)} />);
    await screen.findByRole("button", { name: "Open: Station 0" });
    await waitFor(() => expect(screen.getByRole("grid")).toHaveAttribute("aria-rowcount", "10001"));
    fireEvent.click(await screen.findByRole("button", { name: "Page 200" }));
    const last = await screen.findByRole("button", { name: "Open: Station 9999" });
    expect(screen.queryByRole("button", { name: "Open: Station 0" })).toBeNull();
    // A screen reader hears the row's place in the whole set, not on the page.
    expect(last.closest("[role=row]")).toHaveAttribute("aria-rowindex", "10001");
  });

  it("a visitor whose search matches nothing is told so in words and gets every row back in one click", async () => {
    const inner = fixtureSource([station(1), station(2)]);
    const source: EntitySource = { ...inner, query: (q, page) => (q.q ? Promise.resolve({ rows: [], total: 0 }) : inner.query(q, page)) };
    render(<EntityGrid config={configOf([{ attr: "name", label: "Name" }])} source={source} />);
    await screen.findByRole("button", { name: "Open: Station 1" });
    fireEvent.change(screen.getByLabelText("Filter: Name"), { target: { value: "contains" } });
    fireEvent.change(screen.getByLabelText("Value: Name"), { target: { value: "zzz" } });
    expect(await screen.findByText(DEFAULT_LABELS.filtersMatchNothing)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.clearFilters }));
    await screen.findByRole("button", { name: "Open: Station 1" });
    expect(screen.getByLabelText("Filter: Name")).toHaveValue("");
    expect(screen.queryByText(DEFAULT_LABELS.filtersMatchNothing)).toBeNull();
  });

  it("a city analyst combines two filters, reads both as a summary and clears them at once", async () => {
    const inner = fixtureSource([station(1, { bikes: p(5), status: p("open") })]);
    const asked: (string | undefined)[] = [];
    const source: EntitySource = { ...inner, query: (q: GridQuery, page) => (asked.push(q.q), inner.query(q, page)) };
    render(<EntityGrid config={configOf([...NAME_BIKES, { attr: "status", label: "Status" }])} source={source} />);
    await screen.findByRole("button", { name: "Open: Station 1" });
    fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
    fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "4" } });
    fireEvent.change(screen.getByLabelText("Filter: Status"), { target: { value: "equals" } });
    fireEvent.change(screen.getByLabelText("Value: Status"), { target: { value: "open" } });
    await waitFor(() => expect(asked.at(-1)).toBe('bikes>4;status=="open"'));
    const summary = screen.getByRole("list", { name: DEFAULT_LABELS.activeFilters });
    expect(within(summary).getByText("Bikes is greater than 4")).toBeInTheDocument();
    expect(within(summary).getByText("Status is open")).toBeInTheDocument();
    fireEvent.click(within(summary).getByRole("button", { name: DEFAULT_LABELS.clearFilters }));
    await waitFor(() => expect(asked.at(-1)).toBeUndefined());
    expect(screen.queryByRole("list", { name: DEFAULT_LABELS.activeFilters })).toBeNull();
  });

  it("two filters that match nothing offer one way to clear them, not two", async () => {
    const inner = fixtureSource([station(1, { bikes: p(5), status: p("open") })]);
    const source: EntitySource = { ...inner, query: (q, page) => (q.q ? Promise.resolve({ rows: [], total: 0 }) : inner.query(q, page)) };
    render(<EntityGrid config={configOf([...NAME_BIKES, { attr: "status", label: "Status" }])} source={source} />);
    await screen.findByRole("button", { name: "Open: Station 1" });
    fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
    fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "9" } });
    fireEvent.change(screen.getByLabelText("Filter: Status"), { target: { value: "equals" } });
    fireEvent.change(screen.getByLabelText("Value: Status"), { target: { value: "closed" } });
    expect(await screen.findByText(DEFAULT_LABELS.filtersMatchNothing)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: DEFAULT_LABELS.clearFilters })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.clearFilters }));
    await screen.findByRole("button", { name: "Open: Station 1" });
  });
});

describe("ordering with gaps (T-7063)", () => {
  const rows = [
    station(1, { bikes: p(5), seen: p("2026-10-01") }),
    station(2),
    station(3, { bikes: p(3), seen: p("2026-10-03") }),
  ];
  const columns = [...NAME_BIKES, { attr: "seen", label: "Seen" }];

  it("sorting this page by a number or a date keeps a station without the value last, either way", async () => {
    render(<EntityGrid config={configOf(columns)} source={{ ...fixtureSource(rows), orders: false }} />);
    await screen.findByRole("button", { name: "Open: Station 1" });
    const sort = (label: string) => fireEvent.click(screen.getByRole("button", { name: `${DEFAULT_LABELS.sortPage} ${label}` }));
    sort("Bikes");
    await waitFor(() => expect(shownNames()).toEqual(["Station 3", "Station 1", "Station 2"]));
    sort("Bikes");
    await waitFor(() => expect(shownNames()).toEqual(["Station 1", "Station 3", "Station 2"]));
    sort("Seen");
    await waitFor(() => expect(shownNames()).toEqual(["Station 1", "Station 3", "Station 2"]));
    sort("Seen");
    await waitFor(() => expect(shownNames()).toEqual(["Station 3", "Station 1", "Station 2"]));
  });

  it("sorting every row by a date asks the source, which keeps the undated last", async () => {
    render(<EntityGrid config={configOf(columns)} source={fixtureSource(rows)} />);
    await screen.findByRole("button", { name: "Open: Station 1" });
    const button = () => screen.getByRole("button", { name: `${DEFAULT_LABELS.sortAll} Seen` });
    fireEvent.click(button());
    await waitFor(() => expect(shownNames()).toEqual(["Station 1", "Station 3", "Station 2"]));
    fireEvent.click(button());
    await waitFor(() => expect(shownNames()).toEqual(["Station 3", "Station 1", "Station 2"]));
  });

  it("sorting this page by the entity's own timestamp sorts by it, not by an empty text", async () => {
    const stamped = [1, 2, 3].map((n) => ({ ...station(n), modifiedAt: `2026-10-0${4 - n}T00:00:00Z` }));
    render(<EntityGrid config={configOf([{ attr: "name", label: "Name" }], { entityTimestamps: true })} source={{ ...fixtureSource(stamped), orders: false }} />);
    await screen.findByRole("button", { name: "Open: Station 1" });
    fireEvent.click(screen.getByRole("button", { name: `${DEFAULT_LABELS.sortPage} ${DEFAULT_LABELS.modifiedAt}` }));
    await waitFor(() => expect(shownNames()).toEqual(["Station 3", "Station 2", "Station 1"]));
  });
});

describe("going across entities (T-7063)", () => {
  it("a person follows a station's owner and comes back to the rows and the filter they left", async () => {
    const companies = [1, 2].map((n) => ({ id: `urn:ngsi-ld:Company:c${n}`, type: "Company", name: p(`Company ${n}`) }));
    const rows = [1, 2].map((n) => station(n, { bikes: p(n), owner: { type: "Relationship", object: `urn:ngsi-ld:Company:c${n}` } }));
    render(<EntityGrid config={configOf([...NAME_BIKES, { attr: "owner", label: "Owner" }])} source={fixtureSource([...rows, ...companies])} />);
    await screen.findByRole("button", { name: "Open: Station 1" });
    fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
    fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "0" } });
    fireEvent.click((await screen.findAllByRole("button", { name: `${DEFAULT_LABELS.openTarget}: Company · Company 1` }))[0]);
    const trail = await screen.findByRole("navigation", { name: DEFAULT_LABELS.drillTrail });
    fireEvent.click(within(trail).getByRole("button", { name: `← ${DEFAULT_LABELS.drillBack}` }));
    await waitFor(() => expect(screen.queryByRole("navigation", { name: DEFAULT_LABELS.drillTrail })).toBeNull());
    expect(screen.getByLabelText("Filter: Bikes")).toHaveValue("gt");
    expect(screen.getByLabelText("Value: Bikes")).toHaveValue(0);
    expect(screen.getByRole("button", { name: "Open: Station 1" })).toBeInTheDocument();
  });
});

describe("a slow or failing broker (T-7063)", () => {
  it("a visitor on a slow broker reads that the rows are loading, never an empty table", async () => {
    const inner = fixtureSource([station(1)]);
    let release!: () => void;
    const gate = new Promise<void>((done) => (release = done));
    const source: EntitySource = {
      ...inner,
      query: async (q, page) => {
        if (page.limit > 0) await gate;
        return inner.query(q, page);
      },
    };
    render(<EntityGrid config={configOf([{ attr: "name", label: "Name" }])} source={source} />);
    expect(await screen.findByText(DEFAULT_LABELS.loading)).toHaveAttribute("role", "status");
    expect(screen.queryByText(DEFAULT_LABELS.empty)).toBeNull();
    release();
    await screen.findByRole("button", { name: "Open: Station 1" });
    expect(screen.queryByText(DEFAULT_LABELS.loading)).toBeNull();
  });

  it("a visitor whose connection drops is told so in plain words and can try again", async () => {
    const inner = fixtureSource([station(1)]);
    let fail = true;
    const source: EntitySource = {
      ...inner,
      query: (q, page) => (fail ? Promise.reject(new SourceError(0, "TypeError: Failed to fetch")) : inner.query(q, page)),
    };
    const { container } = render(<EntityGrid config={configOf([{ attr: "name", label: "Name" }])} source={source} />);
    const retry = await screen.findByRole("button", { name: DEFAULT_LABELS.retry });
    const line = container.querySelector(".jc-grid-error")!;
    expect(line).toHaveTextContent(DEFAULT_LABELS.unreachable);
    // The raw sentence is kept, but behind the disclosure, not in the line a person reads first.
    expect(line.querySelector("details")).toHaveTextContent("Failed to fetch");
    fail = false;
    fireEvent.click(retry);
    await screen.findByRole("button", { name: "Open: Station 1" });
  });
});

describe("reading and writing safely (T-7063)", () => {
  it("an anonymous visitor finds no way to change anything, even where the config asks for edit", async () => {
    const patch = vi.fn(async () => undefined);
    const config = configOf(NAME_BIKES, { mode: "edit", editableAttrs: ["name", "bikes"] });
    render(<EntityGrid config={config} source={{ ...fixtureSource([station(1, { bikes: p(5) })]), patch }} publicView />);
    await screen.findByRole("button", { name: "Open: Station 1" });
    expect(screen.queryAllByLabelText(/^Edit (Name|Bikes)$/)).toEqual([]);
    expect(screen.queryByRole("button", { name: DEFAULT_LABELS.review })).toBeNull();
    fireEvent.paste(screen.getByRole("grid"), { clipboardData: { getData: () => "9" } });
    expect(screen.queryByText(new RegExp(DEFAULT_LABELS.pending))).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open: Station 1" }));
    const dialog = await screen.findByRole("dialog", { name: "Details: Station 1" });
    expect(within(dialog).queryAllByRole("textbox")).toEqual([]);
    expect(within(dialog).queryAllByRole("spinbutton")).toEqual([]);
    expect(patch).not.toHaveBeenCalled();
  });

  it("an editor refused by the server reads why by the station's name, and gets the stored value back in one click", async () => {
    const config = configOf(NAME_BIKES, { mode: "edit", editableAttrs: ["bikes"] });
    const source = {
      ...fixtureSource([station(1, { bikes: p(5) })]),
      patch: async () => {
        throw new SourceError(403, "Access Denied by Policy");
      },
    };
    render(<EntityGrid config={config} source={source} />);
    fireEvent.change(await screen.findByDisplayValue("5"), { target: { value: "4" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    fireEvent.click(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply }));
    expect(await within(panel).findByText(`Station 1 — ${DEFAULT_LABELS.notSaved}: Access Denied by Policy`)).toBeInTheDocument();
    expect(panel.textContent).not.toContain("urn:");
    expect(within(panel).getByText(DEFAULT_LABELS.refusedNext)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.discard }));
    expect(await screen.findByDisplayValue("5")).toBeInTheDocument();
  });
});

describe("the person's own view (T-7063)", () => {
  it("a person hides a column and it stays hidden on the next page", async () => {
    const config = configOf(NAME_BIKES, { pageSize: 1 });
    render(<EntityGrid config={config} source={fixtureSource([station(1, { bikes: p(1) }), station(2, { bikes: p(2) })])} />);
    await screen.findByRole("button", { name: "Open: Station 1" });
    fireEvent.click(screen.getByRole("button", { name: `${DEFAULT_LABELS.columnMenu}: Bikes` }));
    fireEvent.click(screen.getByRole("menuitem", { name: DEFAULT_LABELS.hideColumn }));
    await waitFor(() => expect(screen.queryByRole("button", { name: `${DEFAULT_LABELS.columnMenu}: Bikes` })).toBeNull());
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.next }));
    await screen.findByRole("button", { name: "Open: Station 2" });
    expect(screen.queryByRole("button", { name: `${DEFAULT_LABELS.columnMenu}: Bikes` })).toBeNull();
    expect(screen.getByRole("button", { name: `${DEFAULT_LABELS.hiddenFields} (1)` })).toBeInTheDocument();
  });

  it("a keyboard user opens a row from the grid, hears what each control is, and comes back to the same cell", async () => {
    render(<EntityGrid config={configOf(NAME_BIKES)} source={fixtureSource([station(1, { bikes: p(1) }), station(2, { bikes: p(2) })])} />);
    await screen.findByRole("button", { name: "Open: Station 1" });
    const grid = screen.getByRole("grid");
    // Named for a screen reader: the sort, the filter and the column menu say which column they act on.
    expect(screen.getByRole("button", { name: `${DEFAULT_LABELS.sortAll} Bikes` })).toBeInTheDocument();
    expect(screen.getByLabelText("Filter: Bikes")).toBeInTheDocument();
    grid.focus();
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    const cell = grid.getAttribute("aria-activedescendant");
    expect(cell).toBeTruthy();
    fireEvent.keyDown(grid, { key: "Enter" });
    const open = screen.getByRole("button", { name: "Open: Station 1" });
    expect(open).toHaveFocus();
    // Enter on a focused button is the browser's click.
    fireEvent.click(open);
    const heading = await screen.findByRole("heading", { name: "Details: Station 1" });
    fireEvent.keyDown(heading, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(grid).toHaveFocus();
    expect(grid.getAttribute("aria-activedescendant")).toBe(cell);
  });
});
