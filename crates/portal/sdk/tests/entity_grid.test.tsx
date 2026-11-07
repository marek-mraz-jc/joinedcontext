import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, fireEvent, within } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { SourceError, endpointSource, fixtureSource } from "../src/grid/source";
import { DEFAULT_LABELS, VIRTUAL_PAGE_SIZE, useEntityGrid } from "../src/grid/useEntityGrid";
import { formatDate } from "../src/grid/cells";

const bikeEntities: Record<string, unknown>[] = [
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
    type: "BikeHireDockingStation",
    availableBikeNumber: { type: "Property", value: 5, unitCode: "C62", observedAt: "2026-01-01T10:00:00Z" },
    refDevice: { type: "Relationship", object: "urn:ngsi-ld:Device:hel:001" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.93, 60.17] } },
    name: { type: "LanguageProperty", languageMap: { en: "Kamppi", fi: "Kamppi" } },
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:002",
    type: "BikeHireDockingStation",
    availableBikeNumber: { type: "Property", value: 3, unitCode: "C62", observedAt: "2026-01-01T11:00:00Z" },
    refDevice: { type: "Relationship", object: "urn:ngsi-ld:Device:hel:002" },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.95, 60.18] } },
    name: { type: "LanguageProperty", languageMap: { en: "Kallio", fi: "Kallio" } },
  },
];

const configResult = parseGridConfig({
  source: { kind: "fixture", name: "test" },
  type: "BikeHireDockingStation",
  columns: [
    { attr: "name", label: "Name" },
    { attr: "availableBikeNumber", label: "Bikes", show: { unit: true } },
    { attr: "refDevice", label: "Device" },
    { attr: "location", label: "Location" },
  ],
  pageSize: 10,
});

const config = configResult.config!;
// One row a page, so the pager has somewhere to go.
const onePerPage = { ...config, pageSize: 1 };

describe("EntityGrid", () => {
  it("leaves a column without a filter without a header cell in the filter row", async () => {
    const some = { ...config, filters: { allowed: ["availableBikeNumber"], preset: {} } };
    const { container } = render(<EntityGrid config={some} source={fixtureSource(bikeEntities)} />);
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    const row = container.querySelector("tr.jc-grid-filter-row") as HTMLTableRowElement;
    // Every header cell of the filter row holds a filter; the other columns get a plain cell.
    expect(Array.from(row.querySelectorAll("th")).every((cell) => cell.childElementCount > 0)).toBe(true);
    expect(row.querySelectorAll("td").length).toBeGreaterThan(0);
    expect(row.cells).toHaveLength(container.querySelectorAll("thead tr:first-child > *").length);
  });

  it("leaves out the attributes a view hides, and marks a row with its tone and the reason", async () => {
    render(
      <EntityGrid
        config={config}
        source={fixtureSource(bikeEntities)}
        hidden={["availableBikeNumber"]}
        rowTone={(row) => (row.id === bikeEntities[0].id ? { tone: "danger", label: "No bikes left" } : undefined)}
      />,
    );
    await waitFor(() => expect(screen.getAllByRole("row").length).toBeGreaterThan(1));
    expect(screen.queryByRole("columnheader", { name: /availableBikeNumber/ })).toBeNull();
    const marked = screen.getAllByRole("img", { name: "No bikes left" });
    expect(marked).toHaveLength(1);
    expect(marked[0].closest("tr")).toHaveClass("jc-grid-tr--danger");
  });

  it("tells the host the query its filters ask, as it changes, for a saved view to keep", async () => {
    const onQuery = vi.fn();
    const { rerender } = render(<EntityGrid config={config} source={fixtureSource(bikeEntities)} onQuery={onQuery} />);
    await waitFor(() => expect(onQuery).toHaveBeenLastCalledWith({ q: undefined, idPattern: undefined }));
    rerender(
      <EntityGrid config={config} source={fixtureSource(bikeEntities)} onQuery={onQuery} state={{ filterText: "availableBikeNumber<3" }} />,
    );
    await waitFor(() => expect(onQuery).toHaveBeenLastCalledWith({ q: "availableBikeNumber<3", idPattern: undefined }));
  });

  it("renders value with unit", async () => {
    render(<EntityGrid config={config} source={fixtureSource(bikeEntities)} />);
    await waitFor(() => {
      expect(screen.getByText("5")).toBeInTheDocument();
      expect(screen.getByText("3")).toBeInTheDocument();
    });
  });

  it("writes a value with its unit's symbol and names the unit on hover (DM-06)", async () => {
    const air = parseGridConfig({
      source: { kind: "fixture", name: "air" },
      type: "AirQualityObserved",
      columns: [{ attr: "pm10", label: "PM10" }],
      pageSize: 10,
    }).config!;
    const station = {
      id: "urn:ngsi-ld:AirQualityObserved:hel:helsinki:1",
      type: "AirQualityObserved",
      pm10: { type: "Property", value: 12, unitCode: "GQ" },
    };
    render(<EntityGrid config={air} source={fixtureSource([station])} />);
    const cell = await screen.findByText("12 µg/m³");
    expect(cell.closest("td")).toHaveAttribute("title", "microgram per cubic metre (GQ)");
    expect(screen.getByRole("columnheader", { name: /PM10 \(µg\/m³\)/ })).toBeInTheDocument();
  });

  it("toggles observedAt column via header menu", async () => {
    render(<EntityGrid config={config} source={fixtureSource(bikeEntities)} />);
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });

    // Click the menu button for availableBikeNumber
    const menuBtn = screen.getByLabelText("Column options: Bikes");
    fireEvent.click(menuBtn);

    // Check the observedAt checkbox
    const observedCheckbox = screen.getByLabelText("Observed");
    expect(observedCheckbox).not.toBeChecked();
    fireEvent.click(observedCheckbox);

    // Now the observedAt column should appear
    await waitFor(() => {
      // An observed time reads in the reader's own locale and zone (T-4827).
      expect(screen.getByText(formatDate("2026-01-01T10:00:00Z", navigator.language))).toBeInTheDocument();
      expect(screen.getByText(formatDate("2026-01-01T11:00:00Z", navigator.language))).toBeInTheDocument();
    });
  });

  it("names a readable relationship target and opens it; an unreadable one is its id alone", async () => {
    const onOpen = vi.fn();
    const counter = { id: "urn:ngsi-ld:Device:hel:001", type: "Device", name: { type: "Property", value: "Counter 1" } };
    render(
      <EntityGrid
        config={config}
        source={fixtureSource([...bikeEntities, counter])}
        onOpenRelationship={onOpen}
      />,
    );
    // The page's targets are read once, and the chip then says the target's name (G-R2GRID2-017).
    const relBtn = await screen.findByRole("button", { name: `${DEFAULT_LABELS.openTarget}: Device · Counter 1` });
    expect(relBtn).toHaveAttribute("title", "urn:ngsi-ld:Device:hel:001");
    fireEvent.click(relBtn);
    expect(onOpen).toHaveBeenCalledWith("urn:ngsi-ld:Device:hel:001");
  });

  it("shows empty label when source has no rows", async () => {
    render(<EntityGrid config={config} source={fixtureSource([])} />);
    await waitFor(() => {
      expect(screen.getByText("No rows.")).toBeInTheDocument();
    });
  });

  it("moves data-active with ArrowRight, ArrowDown, Home, End, PageDown", async () => {
    render(<EntityGrid config={config} source={fixtureSource(bikeEntities)} />);
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });

    const grid = screen.getByRole("grid");
    grid.focus();

    // ArrowDown
    fireEvent.keyDown(grid, { key: "ArrowDown" });
    await waitFor(() => {
      // second row should have data-active
      const cells = grid.querySelectorAll('[data-active]');
      expect(cells.length).toBeGreaterThan(0);
    });

    // ArrowRight
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    // Home
    fireEvent.keyDown(grid, { key: "Home" });
    // End
    fireEvent.keyDown(grid, { key: "End" });
    // PageDown
    fireEvent.keyDown(grid, { key: "PageDown" });
  });

  // T-3097, UI-70: one tab stop, the active cell announced as the grid's active descendant, its
  // place counted in the whole set, Ctrl+End to the last cell, Enter into the cell's own editor and
  // Escape back to the grid.
  it("names the active cell to assistive technology and moves into and out of its editor", async () => {
    const editable = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "BikeHireDockingStation",
      columns: [
        { attr: "name", label: "Name" },
        { attr: "availableBikeNumber", label: "Bikes" },
      ],
      pageSize: 1,
      mode: "edit",
      editableAttrs: ["availableBikeNumber"],
    }).config!;
    const source = { ...fixtureSource(bikeEntities), patch: vi.fn() };
    render(<EntityGrid config={editable} source={source} />);
    await screen.findByText("Kamppi");
    const grid = screen.getByRole("grid");
    expect(grid).toHaveAttribute("aria-rowcount", "3");
    grid.focus();

    fireEvent.keyDown(grid, { key: "End", ctrlKey: true });
    const active = grid.getAttribute("aria-activedescendant");
    expect(active).toBeTruthy();
    const cell = document.getElementById(active!)!;
    expect(cell).toHaveAttribute("role", "gridcell");
    expect(cell).toHaveAttribute("data-active");

    fireEvent.keyDown(grid, { key: "Enter" });
    const input = cell.querySelector("input")!;
    expect(input).toHaveFocus();
    fireEvent.keyDown(input, { key: "Escape" });
    expect(grid).toHaveFocus();

    // Page two counts from the whole set: the row is the second of two, after the header.
    fireEvent.click(screen.getByRole("button", { name: /next/i }));
    await screen.findByText("Kallio");
    expect(screen.getByText("Kallio").closest("[role=row]")).toHaveAttribute("aria-rowindex", "3");
  });

  // T-3097: the identifier opens the row whole, every attribute with the grid's own editors, and
  // Escape goes back to the grid.
  it("opens a row's detail panel from its identifier, edits there, and closes back to the grid", async () => {
    const two = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "BikeHireDockingStation",
      columns: [
        { attr: "name", label: "Name" },
        { attr: "availableBikeNumber", label: "Bikes" },
      ],
      pageSize: 10,
      mode: "edit",
      editableAttrs: ["availableBikeNumber"],
    }).config!;
    const source = { ...fixtureSource(bikeEntities), patch: vi.fn() };
    render(<EntityGrid config={two} source={source} />);
    await screen.findByText("Kamppi");

    fireEvent.click(screen.getByRole("button", { name: "Open: Kamppi" }));
    const panel = screen.getByRole("dialog", { name: "Details: Kamppi" });
    expect(screen.getByRole("heading", { name: "Details: Kamppi" })).toHaveFocus();
    // The grid's columns, then what the entity carries beyond them.
    const terms = Array.from(panel.querySelectorAll("dt")).map((dt) => dt.textContent);
    expect(terms).toEqual(["Name", "Bikes", "location", "refDevice"]);

    const bikes = panel.querySelector<HTMLInputElement>("dd input")!;
    fireEvent.change(bikes, { target: { value: "7" } });
    expect(within(panel).getByText("1 not applied yet")).toBeInTheDocument();

    fireEvent.keyDown(screen.getByRole("heading", { name: "Details: Kamppi" }), { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("grid")).toHaveFocus();
  });

  // T-3097: a range pasted from a spreadsheet lands on the editable cells from the active one and
  // says what it skipped; nothing is pending outside edit mode.
  it("pastes a copied range from the active cell into editable cells and says what it skipped", async () => {
    const editable = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "BikeHireDockingStation",
      columns: [
        { attr: "name", label: "Name" },
        { attr: "availableBikeNumber", label: "Bikes" },
      ],
      pageSize: 10,
      mode: "edit",
      editableAttrs: ["availableBikeNumber"],
    }).config!;
    const source = { ...fixtureSource(bikeEntities), patch: vi.fn() };
    render(<EntityGrid config={editable} source={source} />);
    await screen.findByText("Kamppi");
    const grid = screen.getByRole("grid");
    grid.focus();
    // id, type, name, bikes: the first key lands on the id, three more to the right is the bikes column.
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.paste(grid, { clipboardData: { getData: () => "8\tignored\n4\n" } });

    expect(screen.getByText("2 cells pasted, 1 skipped (not editable, off the page or not a listed value)")).toBeInTheDocument();
    expect(screen.getByText("2 not applied yet")).toBeInTheDocument();
    expect(screen.getByDisplayValue("8")).toBeInTheDocument();
    expect(screen.getByDisplayValue("4")).toBeInTheDocument();
  });

  // T-3106: the host adds what belongs under a row, such as its comments, to the panel.
  it("draws what the host adds under the opened row's attributes", async () => {
    const config = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "BikeHireDockingStation",
      columns: [{ attr: "name", label: "Name" }],
      pageSize: 10,
    }).config!;
    render(
      <EntityGrid
        config={config}
        source={fixtureSource(bikeEntities)}
        detailExtra={(row) => <p>{`notes on ${row.id}`}</p>}
      />,
    );
    await screen.findByText("Kamppi");
    expect(screen.queryByText(/^notes on/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open: Kamppi" }));
    const panel = screen.getByRole("dialog", { name: "Details: Kamppi" });
    expect(panel).toHaveTextContent(/notes on urn:ngsi-ld:BikeHireDockingStation:/);
  });

  // T-3097, ADR-N-042: a pinned column is the primary field. It takes the identifier's place as the
  // first column, its value opens the row (the id on hover and in the panel), and in the panel it is
  // edited like any other attribute.
  it("keeps id, type and name first whatever the config pins, opens the row from the id and edits it in the panel", async () => {
    const primary = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "BikeHireDockingStation",
      columns: [
        { attr: "availableBikeNumber", label: "Bikes" },
        { attr: "name", label: "Name", pinned: true },
      ],
      pageSize: 10,
      mode: "edit",
      editableAttrs: ["name", "availableBikeNumber"],
    }).config!;
    const source = { ...fixtureSource(bikeEntities), patch: vi.fn() };
    render(<EntityGrid config={primary} source={source} />);
    const open = await screen.findByRole("button", { name: "Open: Kamppi" });
    const headers = screen.getAllByRole("columnheader").map((th) => th.textContent ?? "");
    // T-6210: id, type, name lead every grid, then the rest as the config lists it.
    expect(headers[0]).toMatch(/^ID/);
    expect(headers[1]).toMatch(/^Type/);
    expect(headers[2]).toMatch(/^Name/);
    expect(headers[3]).toMatch(/^Bikes/);
    expect(open).toHaveAttribute("title", "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001");
    expect(open.closest("td")).toHaveClass("jc-grid-pinned");

    fireEvent.click(open);
    const panel = screen.getByRole("dialog", { name: "Details: Kamppi" });
    // T-5599: the id is behind Copy identifier, never text in the dialog.
    expect(panel.textContent).not.toMatch(/urn:/);
    expect(within(panel).getByRole("button", { name: "Copy identifier" })).toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: /^Open/ })).toBeNull();
    expect(within(panel).getAllByRole("textbox").length).toBeGreaterThan(0);
  });

  // T-4885: the footer's total comes from the same answer as the rows; while the first page is on
  // its way it says so, and an answer with no total (the gateway narrowed it, R22) is named in words.
  it("footer: loading, then the endpoint's total, or words for an answer without one", async () => {
    const rows = [{ id: "urn:ngsi-ld:BikeHireDockingStation:a", type: "BikeHireDockingStation", name: { type: "Property", value: "Kamppi" } }];
    const config = parseGridConfig({
      source: { kind: "fixture", name: "few" },
      type: "BikeHireDockingStation",
      columns: [{ attr: "name", label: "Name" }],
      pageSize: 50,
    }).config!;
    const counted = fixtureSource(rows);
    let release: () => void = () => undefined;
    const slow = { ...counted, query: (q: Parameters<typeof counted.query>[0], page: Parameters<typeof counted.query>[1]) =>
      new Promise<Awaited<ReturnType<typeof counted.query>>>((done) => { release = () => void counted.query(q, page).then(done); }) };
    const { container, unmount } = render(<EntityGrid config={config} source={slow} virtual />);
    await waitFor(() => expect(container.querySelector(".jc-grid-total")).toHaveTextContent(DEFAULT_LABELS.loading));
    release();
    await screen.findByText("1 of 1 loaded");
    unmount();

    const narrowed = { ...counted, query: async (q: Parameters<typeof counted.query>[0], page: Parameters<typeof counted.query>[1]) =>
      ({ ...(await counted.query(q, page)), total: undefined }) };
    render(<EntityGrid config={config} source={narrowed} virtual />);
    await screen.findByText(`1 loaded · ${DEFAULT_LABELS.noTotal}`);
  });

  it("shows the rows when the count fails, as a count cut by the gateway's 10 s does (T-6541)", async () => {
    // Praha Tree on dev: `count=true` answered 503 after 10 s while the rows alone took 0.2 s.
    const config = parseGridConfig({
      source: { kind: "fixture", name: "few" },
      type: "BikeHireDockingStation",
      columns: [{ attr: "name", label: "Name" }],
      pageSize: 50,
    }).config!;
    const fixture = fixtureSource(bikeEntities);
    const cut = { ...fixture, query: async (q: Parameters<typeof fixture.query>[0], page: Parameters<typeof fixture.query>[1]) => {
      if (page.count !== false) throw new SourceError(503, "the read took longer than the 10 s a read may take");
      return { ...(await fixture.query(q, page)), total: undefined };
    } };
    render(<EntityGrid config={config} source={cut} virtual />);
    await screen.findByText("Kamppi");
    await screen.findByText(`${bikeEntities.length} loaded · ${DEFAULT_LABELS.noTotal}`);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  // T-3097: a scrolling grid draws the rows in sight and loads the next page as the person nears
  // the end; the footer counts what is loaded of the whole set, and the rows keep their places.
  // T-5623: the next page is asked for once the window passes 60 % of the loaded rows, and it is
  // VIRTUAL_PAGE_SIZE rows, so the first page stays small and the scroll needs few reads.
  it("scrolls through more rows than a page, asking for the next page before the end", async () => {
    const many = Array.from({ length: 1000 }, (_, i) => ({
      id: `urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:${String(i).padStart(4, "0")}`,
      type: "BikeHireDockingStation",
      name: { type: "Property", value: `Station ${i}` },
    }));
    const scrolling = parseGridConfig({
      source: { kind: "fixture", name: "many" },
      type: "BikeHireDockingStation",
      columns: [{ attr: "name", label: "Name" }],
      pageSize: 50,
    }).config!;
    const fixture = fixtureSource(many);
    const asked: { offset: number; limit: number; count?: boolean }[] = [];
    const logged = { ...fixture, query: (q: Parameters<typeof fixture.query>[0], page: Parameters<typeof fixture.query>[1]) => {
      asked.push(page);
      return fixture.query(q, page);
    } };
    const { container } = render(<EntityGrid config={scrolling} source={logged} virtual />);
    await screen.findByText("Station 0");
    // The first page is the config's size; the window of the first rows is past 60 % of 50, so
    // the next, larger page is asked for at once, with no scroll.
    await screen.findByText("250 of 1000 loaded");
    // The count is a read of its own, with no rows; no page of rows asks for it (T-6161, T-6541).
    expect(asked).toEqual([
      { offset: 0, limit: 0, count: true },
      { offset: 0, limit: 50, count: false },
      { offset: 50, limit: VIRTUAL_PAGE_SIZE, count: false },
    ]);
    expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
    const drawn = () => container.querySelectorAll("tbody tr:not(.jc-grid-spacer)");
    expect(drawn().length).toBeLessThanOrEqual(50);

    // Row 140 of 250 loaded: the window (30 rows in sight, 10 more on each side) ends at row 170,
    // past 60 % (150) and well before the end, and that asks for the rows after 250.
    const box = container.querySelector<HTMLDivElement>(".jc-grid-scroll")!;
    box.scrollTop = 140 * 36;
    fireEvent.scroll(box);
    await screen.findByText("450 of 1000 loaded");
    expect(asked[3]).toEqual({ offset: 250, limit: VIRTUAL_PAGE_SIZE, count: false });
    // The window moved: the first drawn row is 130 rows in, and says so to a screen reader.
    expect(screen.queryByText("Station 0")).toBeNull();
    expect(drawn()[0]).toHaveAttribute("aria-rowindex", "132");
    expect(drawn().length).toBeLessThan(100);
  });

  // T-5623: the critic walk clicked a cell, then scrolled; every scroll was pulled back to the
  // active cell, so 60 scrolls reached 400 rows. A scroll away from the cell stays where it went.
  it("leaves the scroll where the person put it after a cell was made active", async () => {
    const many = Array.from({ length: 300 }, (_, i) => ({
      id: `urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:${String(i).padStart(4, "0")}`,
      type: "BikeHireDockingStation",
      name: { type: "Property", value: `Station ${i}` },
    }));
    const scrolling = parseGridConfig({
      source: { kind: "fixture", name: "many" },
      type: "BikeHireDockingStation",
      columns: [{ attr: "name", label: "Name" }],
      pageSize: 50,
    }).config!;
    const { container } = render(<EntityGrid config={scrolling} source={fixtureSource(many)} virtual />);
    await screen.findByText("250 of 300 loaded");
    const grid = screen.getByRole("grid");
    fireEvent.keyDown(grid, { key: "ArrowDown" });
    fireEvent.keyDown(grid, { key: "ArrowDown" });
    await waitFor(() => expect(grid.getAttribute("aria-activedescendant")).toBeTruthy());
    const box = container.querySelector<HTMLDivElement>(".jc-grid-scroll")!;
    box.scrollTop = 100 * 36;
    fireEvent.scroll(box);
    await screen.findByText("Station 100");
    expect(box.scrollTop).toBe(100 * 36);
    expect(screen.queryByText("Station 1")).toBeNull();
  });

  // T-5623: on dev a name filter that matched nothing never showed the empty state: the columns
  // came from the empty answer, the name filter lost its column, the grid asked again without it,
  // got rows back, asked with it again, and so on every half second.
  it("shows the empty state for a filter that matches nothing, and asks once", async () => {
    const columnless = parseGridConfig({
      source: { kind: "fixture", name: "few" },
      type: "BikeHireDockingStation",
      pageSize: 50,
    }).config!;
    const fixture = fixtureSource(bikeEntities);
    const asked: (string | undefined)[] = [];
    const nothingMatches = { ...fixture, query: (q: Parameters<typeof fixture.query>[0], page: Parameters<typeof fixture.query>[1]) => {
      // The page's relationship targets are their own read, by id (G-R2GRID2-017).
      if (!q.ids) asked.push(q.q);
      return q.q ? Promise.resolve({ rows: [], total: 0 }) : fixture.query(q, page);
    } };
    render(<EntityGrid config={columnless} source={nothingMatches} virtual />);
    await screen.findByText("Kamppi");
    fireEvent.change(screen.getByLabelText("Filter: name"), { target: { value: "contains" } });
    fireEvent.change(screen.getByLabelText("Value: name"), { target: { value: "zz-no-such-station" } });
    await screen.findByText(DEFAULT_LABELS.empty);
    await new Promise((settle) => setTimeout(settle, 50));
    // Each question is asked once for its rows and once for its count (T-6541).
    expect(asked).toEqual([undefined, undefined, 'name[*]~="zz-no-such-station"', 'name[*]~="zz-no-such-station"']);
    // The filter stays where the person can see and clear it.
    expect(screen.getByLabelText("Value: name")).toHaveValue("zz-no-such-station");
  });

  // T-5623: a next page still on its way when the question changes is dropped, and the new
  // question loads its own next pages.
  it("loads more for a new question while a next page of the old one was on its way", async () => {
    const many = Array.from({ length: 600 }, (_, i) => ({
      id: `urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:${String(i).padStart(4, "0")}`,
      type: "BikeHireDockingStation",
      name: { type: "Property", value: `Station ${i}` },
    }));
    const scrolling = parseGridConfig({
      source: { kind: "fixture", name: "many" },
      type: "BikeHireDockingStation",
      columns: [{ attr: "name", label: "Name" }],
      pageSize: 50,
    }).config!;
    const fixture = fixtureSource(many);
    const asked: { q?: string; offset: number }[] = [];
    // The first question's next page never answers: it is the one in flight when the question moves.
    const stalled = { ...fixture, query: (q: Parameters<typeof fixture.query>[0], page: Parameters<typeof fixture.query>[1]) => {
      asked.push({ q: q.q, offset: page.offset });
      return !q.q && page.offset > 0 ? new Promise<never>(() => undefined) : fixture.query(q, page);
    } };
    const { rerender } = render(<EntityGrid config={scrolling} source={stalled} virtual />);
    await screen.findByText("Station 0");
    await waitFor(() => expect(asked).toContainEqual({ q: undefined, offset: 50 }));
    rerender(<EntityGrid config={scrolling} source={stalled} virtual query={{ q: "name~=\"Station\"" }} />);
    await screen.findByText("250 of 600 loaded");
    expect(asked).toContainEqual({ q: 'name~="Station"', offset: 50 });
  });

  // T-5623: a person who changed the question sees the last answer dimmed and the words for it,
  // rather than the old rows as if they answered the new question.
  it("dims the last rows and says Filtering… while a narrowed question is read", async () => {
    const scrolling = parseGridConfig({
      source: { kind: "fixture", name: "few" },
      type: "BikeHireDockingStation",
      columns: [{ attr: "name", label: "Name" }],
      pageSize: 50,
    }).config!;
    const fixture = fixtureSource(bikeEntities);
    let hold = false;
    let release: () => void = () => undefined;
    const slow = { ...fixture, query: (q: Parameters<typeof fixture.query>[0], page: Parameters<typeof fixture.query>[1]) =>
      hold ? new Promise<Awaited<ReturnType<typeof fixture.query>>>((done) => { release = () => done({ rows: [], total: 0 }); }) : fixture.query(q, page) };
    const { rerender } = render(<EntityGrid config={scrolling} source={slow} virtual />);
    await screen.findByText("Kamppi");
    expect(screen.getByRole("grid")).not.toHaveAttribute("aria-busy");

    hold = true;
    rerender(<EntityGrid config={scrolling} source={slow} virtual query={{ q: 'name=="nobody"' }} />);
    expect(await screen.findByText(DEFAULT_LABELS.filtering)).toHaveAttribute("role", "status");
    expect(screen.getByRole("grid")).toHaveAttribute("aria-busy", "true");
    expect(screen.getByText("Kamppi")).toBeInTheDocument();

    release();
    await screen.findByText(DEFAULT_LABELS.empty);
    expect(screen.queryByText(DEFAULT_LABELS.filtering)).toBeNull();
    expect(screen.getByRole("grid")).not.toHaveAttribute("aria-busy");
  });

  it("has role grid with aria-rowcount and headers with aria-colindex", async () => {
    render(<EntityGrid config={config} source={fixtureSource(bikeEntities)} />);
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });

    const grid = screen.getByRole("grid");
    expect(grid).toHaveAttribute("aria-rowcount", "3"); // 2 rows + 1 header
    expect(grid).toHaveAttribute("aria-colcount");

    const headers = grid.querySelectorAll('[role="columnheader"]');
    expect(headers.length).toBeGreaterThan(0);
    headers.forEach((h, i) => {
      expect(h).toHaveAttribute("aria-colindex", String(i + 1));
    });
  });

  it("controlled: pager next calls onStateChange and offset does not change by itself", async () => {
    const onStateChange = vi.fn();
    render(
      <EntityGrid
        config={onePerPage}
        source={fixtureSource(bikeEntities)}
        state={{ offset: 0, activeCell: null, selected: [], shown: {}, sort: null }}
        onStateChange={onStateChange}
      />,
    );
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });

    const nextBtn = screen.getByText("Next");
    fireEvent.click(nextBtn);
    expect(onStateChange).toHaveBeenCalledWith(
      expect.objectContaining({ offset: 1 }),
    );
    // offset should still be 0 because controlled
    expect(screen.getByText("Page 1")).toBeInTheDocument();
  });

  it("uncontrolled works without state props", async () => {
    render(<EntityGrid config={onePerPage} source={fixtureSource(bikeEntities)} />);
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });
    // pager works
    const nextBtn = screen.getByText("Next");
    fireEvent.click(nextBtn);
    await waitFor(() => {
      expect(screen.getByText("Page 2")).toBeInTheDocument();
    });
  });

  it("hook alone shows same rows", async () => {
    function TestComponent() {
      const grid = useEntityGrid({ config, source: fixtureSource(bikeEntities) });
      return (
        <ul>
          {grid.rows.map((r) => (
            <li key={r.id}>{r.id}</li>
          ))}
        </ul>
      );
    }
    render(<TestComponent />);
    await waitFor(() => {
      expect(screen.getByText("urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001")).toBeInTheDocument();
      expect(screen.getByText("urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:002")).toBeInTheDocument();
    });
  });

  it("Slovak labels show no English text", async () => {
    const slovakLabels = {
      id: "ID",
      type: "Typ",
      createdAt: "Vytvorené",
      modifiedAt: "Upravené",
      observedAt: "Pozorované",
      unit: "Jednotka",
      datasetId: "Dataset",
      empty: "Prázdne.",
      loading: "Načítavam…",
      previous: "Predchádzajúca",
      next: "Nasledujúca",
      page: "Strana",
      showMetadata: "Zobraziť metadáta pre",
      error: "Chyba",
      filter: "Filter",
      ops: {
        contains: "obsahuje",
        equals: "je",
        notEquals: "nie je",
        gt: ">",
        gte: "≥",
        lt: "<",
        lte: "≤",
        between: "medzi",
        empty: "je prázdne",
        present: "má hodnotu",
        pattern: "vyhovuje",
        startsWith: "začína na",
        anyOf: "je jedno z",
      },
      value: "Hodnota",
      upperValue: "Horná hodnota",
      query: "Čo sa pýta",
      copyQuery: "Kopírovať dotaz",
      editAsText: "Upraviť ako text",
      filterRow: "Filtre",
      noFilter: "Všetky",
      filterHintText: "Vyberte test a napíšte slová, napr. obsahuje Espoo.",
      filterHintNumber: "Vyberte test a napíšte číslo, napr. väčšie ako 40.",
      filterHintDate: "Vyberte test a zvoľte dátum, napr. po 2026-10-01.",
      filterHintEnum: "Vyberte test; pri „ktorákoľvek z“ zvoľte jednu alebo viac povolených hodnôt:",
      filterHintOther: "Vyberte test, napr. má hodnotu alebo prázdne.",
      filterExampleText: "Trnava",
      filterExampleNumber: "41",
      filterExampleId: "stanica-12",
      editAsTextHint: "Napíšte dopyt NGSI-LD sami",
      exampleLead: "napr.",
      queryAll: "všetky záznamy",
      copied: "Skopírované",
      sortPage: "Zoradiť túto stranu podľa",
      matching: "Vyhovuje: {n}",
      columnMenu: "Možnosti stĺpca",
      sortAsc: "Zoradiť vzostupne",
      areaPick: "Vybrať oblasť na mape",
      areaClear: "Zrušiť oblasť",
      rowsPerPage: "Riadkov na stranu",
      sortDesc: "Zoradiť zostupne",
      filterColumn: "Filtrovať",
      hideColumn: "Skryť pole",
      renameColumn: "Názov v tomto pohľade",
      renameReset: "Názov z modelu",
      columnWidth: "Šírka",
      narrower: "Užší",
      wider: "Širší",
      moveLeft: "Posunúť doľava",
      moveRight: "Posunúť doprava",
      details: "Podrobnosti",
      close: "Zavrieť",
      history: "História",
    };
    render(
      <EntityGrid
        config={config}
        source={fixtureSource(bikeEntities)}
        labels={slovakLabels}
      />,
    );
    await waitFor(() => {
      expect(screen.getByText("Kamppi")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByLabelText("Možnosti stĺpca: Bikes"));
    // Every visible string is the host's: none of the English defaults that differ from Slovak shows.
    // The query hint's example is data (an attribute name such as availableBikeNumber), not a label.
    const page = document.body.cloneNode(true) as HTMLElement;
    page.querySelectorAll(".jc-grid-query-hint code").forEach((code) => code.remove());
    const text = page.textContent ?? "";
    const pairs: [string, string][] = Object.entries(DEFAULT_LABELS).flatMap(([key, english]) =>
      typeof english === "string"
        ? [[key, english] as [string, string]]
        : Object.entries(english).map(([op, name]) => [`${key}.${op}`, name] as [string, string]),
    );
    for (const [key, english] of pairs) {
      const host = key.startsWith("ops.")
        ? slovakLabels.ops[key.slice(4) as keyof typeof slovakLabels.ops]
        : slovakLabels[key as keyof typeof slovakLabels];
      if (english !== host) {
        expect(text, key).not.toContain(english);
      }
    }
    expect(screen.getByText("Strana 1")).toBeInTheDocument();
    expect(screen.getByText("Pozorované")).toBeInTheDocument();
  });

  it("renders <b>x</b> as text, not HTML", async () => {
    const entitiesWithHtml: Record<string, unknown>[] = [
      {
        id: "urn:1",
        type: "T",
        name: { type: "Property", value: "<b>x</b>" },
      },
    ];
    const configHtml = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "T",
      columns: [{ attr: "name" }],
    }).config!;
    render(<EntityGrid config={configHtml} source={fixtureSource(entitiesWithHtml)} />);
    await waitFor(() => {
      const cell = screen.getByText("<b>x</b>");
      expect(cell).toBeInTheDocument();
      expect(cell.querySelector("b")).toBeNull();
    });
  });

  /**
   * The filter row asks the endpoint, never the loaded page (T-1429, UI-66): each case reads the
   * query the source was given, because that is the only thing that decides what a person sees.
   */
  describe("the filter row", () => {
    /** A source that records every query the grid sends it. */
    function recordingSource(entities: Record<string, unknown>[]) {
      const asked: { q?: string; idPattern?: string; offset: number }[] = [];
      const inner = fixtureSource(entities);
      return {
        asked,
        source: {
          query: async (q: Parameters<typeof inner.query>[0], page: Parameters<typeof inner.query>[1]) => {
            asked.push({ q: q.q, idPattern: q.idPattern, offset: page.offset });
            return inner.query(q, page);
          },
          get: inner.get,
        },
      };
    }

    it("sends the query the chosen operator and value compose", async () => {
      const { asked, source } = recordingSource(bikeEntities);
      render(<EntityGrid config={config} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
      fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "4" } });

      await waitFor(() => {
        expect(asked.at(-1)?.q).toBe("availableBikeNumber>4");
      });
      // And it says so under the grid, where a person can copy it.
      expect(screen.getByText("q=availableBikeNumber>4")).toBeInTheDocument();
    });

    // T-4222 (photo audit T-4162): an unset filter read as a bare "—"; it says it lets every row
    // through, in the App's language.
    it("names the unset filter in words, the App's own or the default", async () => {
      const { source } = recordingSource(bikeEntities);
      const { unmount } = render(<EntityGrid config={config} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());
      const unset = (screen.getByLabelText("Filter: Bikes") as HTMLSelectElement).options[0];
      expect(unset.value).toBe("");
      expect(unset.textContent).toBe("Any");
      unmount();

      render(<EntityGrid config={config} source={source} labels={{ noFilter: "Všetky" }} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());
      expect((screen.getByLabelText("Filter: Bikes") as HTMLSelectElement).options[0].textContent).toBe("Všetky");
    });

    // T-4949, UI-16: each filter and the query switch say what to enter, with an example.
    it("hints each filter by what its column holds, and the query switch", async () => {
      const { source } = recordingSource(bikeEntities);
      render(<EntityGrid config={config} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());
      expect(screen.getByLabelText("Filter: Bikes")).toHaveAccessibleDescription(expect.stringContaining(DEFAULT_LABELS.filterHintNumber));
      expect(screen.getByLabelText("Filter: Name")).toHaveAccessibleDescription(DEFAULT_LABELS.filterHintText);
      expect(screen.getByLabelText(DEFAULT_LABELS.editAsText)).toHaveAccessibleDescription(
        `${DEFAULT_LABELS.editAsTextHint}, ${DEFAULT_LABELS.exampleLead} availableBikeNumber>=5.`,
      );
    });

    // T-5415: the hint's example comes from this type's own first row, the empty query reads
    // "all records", and the copy button says it copied.
    it("builds the query hint from the shown rows and copies with a copied state", async () => {
      const writeText = vi.fn(() => Promise.resolve());
      Object.assign(navigator, { clipboard: { writeText } });
      const { source } = recordingSource(bikeEntities);
      const { container } = render(<EntityGrid config={config} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());
      expect(container.querySelector(".jc-grid-query-hint code")?.textContent).toBe("availableBikeNumber>=5");
      expect(container.querySelector(".jc-grid-query-value")?.textContent).toBe(DEFAULT_LABELS.queryAll);
      expect(screen.queryByRole("button", { name: DEFAULT_LABELS.copyQuery })).toBeNull();
      fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
      fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "4" } });
      const copy = await screen.findByRole("button", { name: DEFAULT_LABELS.copyQuery });
      fireEvent.click(copy);
      await waitFor(() => expect(screen.getByRole("button", { name: DEFAULT_LABELS.copied })).toBeInTheDocument());
      expect(writeText).toHaveBeenCalledWith(expect.stringContaining("availableBikeNumber>4"));
    });

    // T-5415: on a phone the filter row folds under one button that counts the active filters.
    it("folds the filter row under a button that says how many filters are on", async () => {
      const { source } = recordingSource(bikeEntities);
      const { container } = render(<EntityGrid config={config} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());
      const toggle = screen.getByRole("button", { name: DEFAULT_LABELS.filterRow });
      expect(toggle).toHaveAttribute("aria-expanded", "false");
      fireEvent.click(toggle);
      expect(toggle).toHaveAttribute("aria-expanded", "true");
      expect(container.querySelector(".jc-grid")).toHaveAttribute("data-filters", "open");
    });

    // T-5093: a host with its own "Query" toggle keeps the box out from between the rows and the count.
    it("leaves the query box out when the host opens it from its own toolbar", async () => {
      const { source } = recordingSource(bikeEntities);
      const { rerender } = render(<EntityGrid config={config} source={source} queryBox={false} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());
      expect(screen.queryByText(DEFAULT_LABELS.copyQuery)).toBeNull();
      expect(screen.queryByLabelText(DEFAULT_LABELS.editAsText)).toBeNull();
      rerender(<EntityGrid config={config} source={source} queryBox />);
      expect(screen.getByLabelText(DEFAULT_LABELS.editAsText)).toBeInTheDocument();
    });

    it("asks nothing until the filter is complete, and starts again at the first page", async () => {
      const { asked, source } = recordingSource(bikeEntities);
      render(<EntityGrid config={onePerPage} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      fireEvent.click(screen.getByText(DEFAULT_LABELS.next));
      await waitFor(() => expect(asked.at(-1)?.offset).toBe(1));

      // The operator alone narrows nothing: the value is still missing.
      fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
      await waitFor(() => expect(asked.at(-1)?.q).toBeUndefined());

      fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "4" } });
      await waitFor(() => {
        expect(asked.at(-1)?.q).toBe("availableBikeNumber>4");
        // Page one, because page two of the narrowed answer may not exist.
        expect(asked.at(-1)?.offset).toBe(0);
      });
    });

    it("asks the id column by pattern rather than by a term of the query", async () => {
      const { asked, source } = recordingSource(bikeEntities);
      render(<EntityGrid config={config} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      fireEvent.change(screen.getByLabelText("Filter: ID"), { target: { value: "pattern" } });
      fireEvent.change(screen.getByLabelText("Value: ID"), { target: { value: "helsinki:002" } });

      await waitFor(() => {
        expect(asked.at(-1)?.idPattern).toBe("helsinki:002");
        expect(asked.at(-1)?.q).toBeUndefined();
      });
      // The endpoint answered about the set, and the grid shows what came back.
      await waitFor(() => expect(screen.queryByText("Kamppi")).toBeNull());
      expect(screen.getByText("Kallio")).toBeInTheDocument();
    });

    it("hands the query over as text, and sends what the person typed", async () => {
      const { asked, source } = recordingSource(bikeEntities);
      render(<EntityGrid config={config} source={source} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
      fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "4" } });
      await waitFor(() => expect(asked.at(-1)?.q).toBe("availableBikeNumber>4"));

      // The switch starts from what the row built, so nothing is lost by taking it over.
      fireEvent.click(screen.getByLabelText(DEFAULT_LABELS.editAsText));
      const field = screen.getByLabelText(DEFAULT_LABELS.query);
      expect(field).toHaveValue("availableBikeNumber>4");

      // And a `q` the row cannot show is exactly what the text field is for.
      fireEvent.change(field, { target: { value: 'availableBikeNumber>4|name=="Kallio"' } });
      await waitFor(() => expect(asked.at(-1)?.q).toBe('availableBikeNumber>4|name=="Kallio"'));
      // With the query in the person's hands the row's controls are gone, so the two cannot
      // disagree about what is being asked.
      expect(screen.queryByLabelText("Filter: Bikes")).toBeNull();
    });

    it("keeps a preset and a filter both true, and offers only the allowed columns", async () => {
      const { asked, source } = recordingSource(bikeEntities);
      render(
        <EntityGrid
          config={{
            ...config,
            filters: { allowed: ["availableBikeNumber"], preset: { q: 'name=="Kamppi"' } },
          }}
          source={source}
        />,
      );
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());
      // Only the allowed column has a filter: a grid narrowed by its dashboard stays narrowed.
      expect(screen.queryByLabelText("Filter: Name")).toBeNull();

      fireEvent.change(screen.getByLabelText("Filter: Bikes"), { target: { value: "gt" } });
      fireEvent.change(screen.getByLabelText("Value: Bikes"), { target: { value: "4" } });
      await waitFor(() => expect(asked.at(-1)?.q).toBe('name=="Kamppi";availableBikeNumber>4'));
    });

    it("shows the endpoint's own count in the footer, and nothing when it sent none", async () => {
      const counted = {
        query: async () => ({ rows: [], total: 41 }),
        get: async () => null,
      };
      const { unmount } = render(<EntityGrid config={config} source={counted} />);
      expect(await screen.findByText(`41 ${DEFAULT_LABELS.matching}`)).toBeInTheDocument();
      unmount();

      // A language whose word agrees with the number places it itself (sk "1 vyhovujúcich" was wrong).
      const placed = render(<EntityGrid config={config} source={counted} labels={{ ...DEFAULT_LABELS, matching: "Vyhovuje: {n}" }} />);
      expect(await screen.findByText("Vyhovuje: 41")).toBeInTheDocument();
      placed.unmount();

      // A narrowed answer carries no count (R22): the footer then pages without claiming a total.
      const uncounted = { query: async () => ({ rows: [] }), get: async () => null };
      render(<EntityGrid config={config} source={uncounted} />);
      await waitFor(() => expect(screen.getByText(DEFAULT_LABELS.empty)).toBeInTheDocument());
      expect(screen.queryByText(new RegExp(DEFAULT_LABELS.matching))).toBeNull();
    });

    it("offers no filter for a geometry, and none for a column of entity timestamps", async () => {
      render(<EntityGrid config={{ ...config, entityTimestamps: true }} source={fixtureSource(bikeEntities)} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      expect(screen.queryByLabelText("Filter: Location")).toBeNull();
      expect(screen.queryByLabelText(`Filter: ${DEFAULT_LABELS.createdAt}`)).toBeNull();
      expect(screen.getByLabelText("Filter: Bikes")).toBeInTheDocument();
    });

    it("says that sorting orders the loaded page", async () => {
      // A source that cannot order the whole answer: the grid sorts what it loaded.
      render(<EntityGrid config={config} source={{ ...fixtureSource(bikeEntities), orders: false }} />);
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      const sort = screen.getByRole("button", { name: `${DEFAULT_LABELS.sortPage} Bikes` });
      fireEvent.click(sort);
      // Ascending by the page's own values: 3 before 5, and the header says which way it went.
      const cells = screen.getAllByRole("gridcell").map((cell) => cell.textContent);
      expect(cells.indexOf("3")).toBeLessThan(cells.indexOf("5"));
      expect(screen.getByRole("button", { name: `${DEFAULT_LABELS.sortPage} Bikes` }).textContent).toContain("↑");
      // A screen reader hears the direction from the header, the arrow being for the eye.
      expect(sort.closest('[role="columnheader"]')).toHaveAttribute("aria-sort", "ascending");
      fireEvent.click(sort);
      expect(sort.closest('[role="columnheader"]')).toHaveAttribute("aria-sort", "descending");
      expect(screen.getByRole("button", { name: `${DEFAULT_LABELS.sortPage} Name` }).closest('[role="columnheader"]')).not.toHaveAttribute("aria-sort");
    });
  });

  // A source that orders the whole answer is asked to (T-5082): the first page holds the first rows
  // of the set, and the header promises every row, not the page.
  describe("sorting through the source", () => {
    it("asks the source for the order, from the first page, and keeps the rows as they came", async () => {
      const asked: { orderBy?: string; offset: number }[] = [];
      const fixture = fixtureSource(bikeEntities);
      const source = {
        ...fixture,
        query: async (q: Parameters<typeof fixture.query>[0], page: { offset: number; limit: number }) => {
          asked.push({ orderBy: q.orderBy, offset: page.offset });
          return fixture.query(q, page);
        },
      };
      render(<EntityGrid config={onePerPage} source={source} />);
      await waitFor(() => expect(asked.length).toBeGreaterThan(0));
      expect(asked[0].orderBy).toBeUndefined();
      fireEvent.click(await screen.findByRole("button", { name: DEFAULT_LABELS.next }));
      await waitFor(() => expect(asked.at(-1)?.offset).toBe(1));

      fireEvent.click(screen.getByRole("button", { name: `${DEFAULT_LABELS.sortAll} Bikes` }));
      await waitFor(() => expect(asked.at(-1)).toEqual({ orderBy: "availableBikeNumber", offset: 0 }));
      const header = screen.getByRole("button", { name: `${DEFAULT_LABELS.sortAll} Bikes` });
      expect(header.closest('[role="columnheader"]')).toHaveAttribute("aria-sort", "ascending");
      fireEvent.click(header);
      await waitFor(() => expect(asked.at(-1)).toEqual({ orderBy: "availableBikeNumber;desc", offset: 0 }));
      // The source's highest comes first: the one row of the page is the most bikes.
      await waitFor(() => expect(screen.getAllByRole("gridcell").map((c) => c.textContent)).toContain("5"));
    });

    // A view that also reads other brokers refuses `orderBy` (CIM 009 5.7.2.4, 4.23.1): the grid asks
    // once more without it, sorts the loaded rows and says so, and never sends the refused order
    // again for that source (T-5618).
    const refusesOrder = () => {
      const paths: string[] = [];
      const transport = async (req: { path: string }) => {
        paths.push(req.path);
        if (req.path.includes("orderBy=")) {
          return {
            status: 400,
            headers: {},
            body: {
              type: "https://uri.etsi.org/ngsi-ld/errors/BadRequestData",
              title: "BadRequestData",
              detail: "orderBy requires local scope — ordering is never applied to distributed operations (5.7.2.4, 4.23.1)",
            },
          };
        }
        return { status: 200, headers: {}, body: bikeEntities };
      };
      return { paths, source: endpointSource("t5618-federating-view", transport as never) };
    };
    // The cell's value, without its inspector button (T-5528).
    const names = () => screen.getAllByRole("gridcell").map((c) => c.querySelector(".jc-grid-cell")?.textContent).filter((t) => t === "Kamppi" || t === "Kallio");

    it("sorts the loaded rows when the source refuses the order, and asks for it only once", async () => {
      const { paths, source } = refusesOrder();
      const { unmount } = render(<EntityGrid config={config} source={source} />);
      await waitFor(() => expect(names()).toEqual(["Kamppi", "Kallio"]));

      fireEvent.click(screen.getByRole("button", { name: `${DEFAULT_LABELS.sortAll} Bikes` }));
      await waitFor(() => expect(names()).toEqual(["Kallio", "Kamppi"]));
      expect(screen.getByText(DEFAULT_LABELS.sortedLoaded.replace("{count}", "2"))).toHaveAttribute("role", "status");
      expect(screen.queryByText(/local scope/)).toBeNull();
      // The header no longer promises every row.
      const header = screen.getByRole("button", { name: `${DEFAULT_LABELS.sortPage} Bikes` });
      expect(paths.filter((path) => path.includes("orderBy=")).length).toBe(1);

      fireEvent.click(header);
      await waitFor(() => expect(names()).toEqual(["Kamppi", "Kallio"]));
      expect(paths.filter((path) => path.includes("orderBy=")).length).toBe(1);

      // The remembered sort reopens with rows, not the refusal.
      unmount();
      render(<EntityGrid config={config} source={source} state={{ sort: { attr: "availableBikeNumber", dir: "asc" } }} />);
      await waitFor(() => expect(names()).toEqual(["Kallio", "Kamppi"]));
      expect(paths.filter((path) => path.includes("orderBy=")).length).toBe(1);
      expect(screen.queryByText(new RegExp(DEFAULT_LABELS.error))).toBeNull();
    });

    it("words any other refused query plainly and keeps the broker's sentence under a disclosure", async () => {
      const transport = async () => ({
        status: 400,
        headers: {},
        body: { type: "https://uri.etsi.org/ngsi-ld/errors/InvalidRequest", title: "InvalidRequest", detail: "q: unexpected token at 7 (4.9)" },
      });
      render(<EntityGrid config={config} source={endpointSource("t5618-bad-query", transport as never)} />);
      expect(await screen.findByText(`${DEFAULT_LABELS.error}: ${DEFAULT_LABELS.queryRefused}: InvalidRequest`, { exact: false })).toBeInTheDocument();
      const disclosure = screen.getByText(DEFAULT_LABELS.errorDetail);
      expect(disclosure.tagName).toBe("SUMMARY");
      expect(disclosure.closest("details")).toHaveTextContent("q: unexpected token at 7 (4.9)");
    });
  });

  // What a host needs to build a page around the grid instead of a table of its own (T-1432): a way
  // to make a row open something, and the page of rows it is looking at.
  describe("what it hands the host", () => {
    it("lets the host draw the identifier, so a row opens what the page keeps", async () => {
      const opened: string[] = [];
      render(
        <EntityGrid
          config={config}
          source={fixtureSource(bikeEntities)}
          renderers={{
            id: (_cell, row) => (
              <button type="button" onClick={() => opened.push(row.id)}>
                {row.id}
              </button>
            ),
          }}
        />,
      );
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());

      fireEvent.click(screen.getByRole("button", { name: bikeEntities[1].id as string }));
      expect(opened).toEqual([bikeEntities[1].id]);
    });

    it("hands over the page it shows, with the offset it starts at", async () => {
      const pages: { ids: string[]; offset: number }[] = [];
      render(
        <EntityGrid
          config={onePerPage}
          source={fixtureSource(bikeEntities)}
          onRows={(rows, offset) => pages.push({ ids: rows.map((row) => row.id), offset })}
        />,
      );
      await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());
      fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.next }));

      // Waited on for its own sake, not for the row to paint (T-2275): the text and the handover are
      // two effects of one render, so asserting after the text can read the call before it happened.
      // This still fails if the handover never comes — `waitFor` ends in the assertion's own failure.
      await waitFor(() =>
        expect(pages.at(-1)).toEqual({ ids: [bikeEntities[1].id], offset: 1 }),
      );
    });
  });
});
