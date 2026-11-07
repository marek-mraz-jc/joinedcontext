/**
 * The grid as parts (T-6350): each part alone with its labels and events, then one test per shape
 * a host composes. The parts draw from one `useGridView`, so a host asks what `EntityGrid` asks.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import React from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../src/geo/GeoView", () => ({
  GeoView: (props: { value?: { id: string }[]; onSelect?: (id: string) => void }) => (
    <div data-testid="geo-view">
      {(props.value ?? []).map((feature) => (
        <button key={feature.id} type="button" onClick={() => props.onSelect?.(feature.id)}>
          {`shape ${feature.id}`}
        </button>
      ))}
    </div>
  ),
}));

const { parseGridConfig } = await import("../src/grid/config");
const { EntityGrid, GridFrame } = await import("../src/grid/EntityGrid");
const { EntityHistory } = await import("../src/grid/EntityHistory");
const { GridDetail, GridCellPanels } = await import("../src/grid/GridDetail");
const { GridHead, moveTo } = await import("../src/grid/GridHead");
const { GridMapPanel } = await import("../src/grid/GridMapPanel");
const { GridNotices, GridStatus, HiddenFields } = await import("../src/grid/GridNotices");
const { GridPager, pageNumbers } = await import("../src/grid/GridPager");
const { GridQueryBox } = await import("../src/grid/GridQueryBox");
const { GridPending, GridReview } = await import("../src/grid/GridReview");
const { GridRows, GridFooter } = await import("../src/grid/GridRows");
const { GridTable } = await import("../src/grid/GridTable");
const { AttributeView, RowDetail } = await import("../src/grid/RowDetail");
const { fixtureSource, SourceError } = await import("../src/grid/source");
const { DEFAULT_LABELS } = await import("../src/grid/useEntityGrid");
const { useGridView } = await import("../src/grid/useGridView");
type EntityGridProps = import("../src/grid/EntityGrid").EntityGridProps;
type GridView = import("../src/grid/useGridView").GridView;

afterEach(cleanup);

const entities = Array.from({ length: 3 }, (_, at) => ({
  id: `urn:ngsi-ld:Station:s${at}`,
  type: "Station",
  name: { type: "Property", value: `Station ${at}` },
  bikes: { type: "Property", value: at },
  location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.9 + at / 100, 60.1] } },
}));

const config = (extra: Record<string, unknown> = {}) =>
  parseGridConfig({
    source: { kind: "endpoint", slug: "stations-aaaaaaaaaaaaaaaaaaaaaaaa" },
    type: "Station",
    columns: [{ attr: "name", label: "Name", pinned: true }, { attr: "bikes", label: "Bikes" }],
    pageSize: 2,
    ...extra,
  }).config!;

const plain = config();
const stations = fixtureSource(entities);

/** A host of its own: asks one view and draws the parts it wants from it. */
function Host({ parts, ...props }: Partial<EntityGridProps> & { parts: (view: GridView) => React.ReactNode }) {
  const view = useGridView({ config: plain, source: stations, ...props });
  return <GridFrame view={view}>{parts(view)}</GridFrame>;
}

const rows = () => screen.findByRole("button", { name: /Station 0/ });

describe("GridTable", () => {
  it("draws the head, the filter row and the rows alone, with no pager, query box or map", async () => {
    render(<Host parts={(view) => <GridTable view={view} />} />);
    await rows();
    expect(screen.getByRole("grid")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: `${DEFAULT_LABELS.sortAll} Name` })).toBeInTheDocument();
    expect(screen.getByRole("row", { name: DEFAULT_LABELS.filterRow })).toBeInTheDocument();
    expect(screen.queryByText(DEFAULT_LABELS.next)).toBeNull();
    expect(screen.queryByText(DEFAULT_LABELS.query)).toBeNull();
  });

  it("sorts from a header and hands the rows to the host", async () => {
    const onRows = vi.fn();
    render(<Host onRows={onRows} parts={(view) => <GridTable view={view} />} />);
    await rows();
    expect(onRows.mock.lastCall?.[0]).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: `${DEFAULT_LABELS.sortAll} Bikes` }));
    await waitFor(() => expect(screen.getByRole("button", { name: `${DEFAULT_LABELS.sortAll} Bikes` }).textContent).toContain("↑"));
  });

  it("offers no filter row when the host asks through its own panel", async () => {
    render(<Host filterRow={false} parts={(view) => <GridTable view={view} />} />);
    await rows();
    expect(screen.queryByRole("row", { name: DEFAULT_LABELS.filterRow })).toBeNull();
  });

  it("draws its head, body and footer as their own parts", async () => {
    render(
      <Host
        footer={(column) => `sum ${column.label}`}
        parts={(view) => (
          <table>
            <GridHead view={view} />
            <GridRows view={view} />
            <GridFooter view={view} />
          </table>
        )}
      />,
    );
    await rows();
    expect(screen.getByText("sum Bikes")).toBeInTheDocument();
    expect(screen.getAllByRole("columnheader").length).toBeGreaterThan(1);
  });

  it("checks rows through the host's own selection", async () => {
    const onSelectedChange = vi.fn();
    render(<Host rowNumbers={{ selectRow: "Pick", selectAll: "Pick all" }} onSelectedChange={onSelectedChange} parts={(view) => <GridTable view={view} />} />);
    await rows();
    fireEvent.click(screen.getByRole("checkbox", { name: "Pick: Station 1" }));
    expect(onSelectedChange).toHaveBeenCalledWith(["urn:ngsi-ld:Station:s1"]);
  });

  it("moves a column by name, every other in its order", () => {
    expect(moveTo(["a", "b", "c"], "c", 0)).toEqual(["c", "a", "b"]);
  });
});

describe("GridPager", () => {
  it("pages with its labels and says the Endpoint's own count", async () => {
    render(<Host parts={(view) => <><GridTable view={view} /><GridPager view={view} /></>} />);
    await rows();
    expect(screen.getByText(`${DEFAULT_LABELS.page} 1`, { exact: false })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: new RegExp(DEFAULT_LABELS.next) }));
    expect(await screen.findByRole("button", { name: /Station 2/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: new RegExp(DEFAULT_LABELS.previous) }));
    expect(await rows()).toBeInTheDocument();
  });

  it("offers the host's page sizes and starts again at the first page on a pick", async () => {
    const onPageSize = vi.fn();
    render(<Host pageSizes={[2, 50]} onPageSize={onPageSize} parts={(view) => <GridPager view={view} />} />);
    await screen.findByText(DEFAULT_LABELS.rowsPerPage);
    fireEvent.change(screen.getByLabelText(DEFAULT_LABELS.rowsPerPage), { target: { value: "50" } });
    expect(onPageSize).toHaveBeenCalledWith(50);
  });

  it("draws the status line of a scrolling grid, and the host can ask for either", async () => {
    render(<Host virtual parts={(view) => <GridPager view={view} />} />);
    expect(await screen.findByRole("status")).toHaveTextContent(DEFAULT_LABELS.loaded);
    cleanup();
    render(<Host parts={(view) => <GridPager view={view} mode="status" />} />);
    expect(await screen.findByRole("status")).toHaveTextContent(DEFAULT_LABELS.loaded);
  });

  it("numbers the first, the last and the pages around this one", () => {
    expect(pageNumbers(5, 9)).toEqual([1, null, 4, 5, 6, null, 9]);
  });
});

describe("GridQueryBox", () => {
  it("says what is asked and hands the query over as text", async () => {
    render(<Host parts={(view) => <GridQueryBox view={view} />} />);
    expect(await screen.findByText(DEFAULT_LABELS.queryAll)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: DEFAULT_LABELS.editAsText }));
    expect(await screen.findByRole("textbox", { name: DEFAULT_LABELS.query })).toBeInTheDocument();
  });

  it("is absent when the host opens the query from its own toolbar", async () => {
    render(<Host queryBox={false} parts={(view) => <><GridTable view={view} /><GridQueryBox view={view} /></>} />);
    await rows();
    expect(screen.queryByText(DEFAULT_LABELS.query)).toBeNull();
  });
});

describe("GridMapPanel", () => {
  const mapped = config({ map: { enabled: true, attr: "location" } });

  it("draws the page's shapes where the config asks for a map", async () => {
    render(<Host config={mapped} parts={(view) => <GridMapPanel view={view} />} />);
    expect(await screen.findByTestId("geo-view")).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: /^shape urn:ngsi-ld:Station:s0/ })).toBeInTheDocument();
  });

  it("is absent on a type or config without one", async () => {
    render(<Host parts={(view) => <><GridTable view={view} /><GridMapPanel view={view} /></>} />);
    await rows();
    expect(screen.queryByTestId("geo-view")).toBeNull();
  });

  it("opens the row of a shape in the shared view", async () => {
    render(<Host config={mapped} parts={(view) => <><GridTable view={view} /><GridMapPanel view={view} /><GridDetail view={view} /></>} />);
    await rows();
    fireEvent.click(await screen.findByRole("button", { name: /^shape urn:ngsi-ld:Station:s2/ }));
    expect(await screen.findByRole("dialog", { hidden: true })).toBeInTheDocument();
  });
});

describe("RowDetail and AttributeView", () => {
  const row = { id: "urn:ngsi-ld:Station:s0", type: "Station", cells: { name: { kind: "property", value: "Kamppi" } } } as never;
  const words = { rowDetail: "Details", close: "Close", rowFields: "Fields", rowHistory: "History", historyAttr: "Attribute" };
  const columns = [{ key: "name", attr: "name", meta: null, label: "Name", pinned: false }];
  beforeDialog();

  it("adds a host's tab after its own and closes", () => {
    const onClose = vi.fn();
    render(<RowDetail row={row} name="Kamppi" columns={columns} renderValue={() => "Kamppi"} labels={words} onClose={onClose} tabs={[{ id: "notes", label: "Notes", content: <p>a note</p> }]} />);
    fireEvent.click(screen.getByRole("tab", { name: "Notes", hidden: true }));
    expect(screen.getByText("a note")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close", hidden: true }));
    expect(onClose).toHaveBeenCalled();
  });

  it("shows one attribute's value, metadata and history, each only where the host has it", () => {
    const { rerender } = render(<AttributeView label="Bikes" value="7" metadata={<p>written by</p>} history={<p>readings</p>} />);
    expect(screen.getByLabelText("Bikes")).toHaveTextContent("7");
    expect(screen.getByText("written by")).toBeInTheDocument();
    expect(screen.getByText("readings")).toBeInTheDocument();
    rerender(<AttributeView label="Bikes" value="7" />);
    expect(screen.queryByText("readings")).toBeNull();
  });
});

describe("EntityHistory", () => {
  it("lets a host swap the picture of one kind", async () => {
    const points = [{ at: "2026-11-01T10:00:00Z", value: 4 }, { at: "2026-11-01T11:00:00Z", value: 5 }];
    render(
      <EntityHistory
        source={{ history: async () => points } as never}
        id="urn:ngsi-ld:Station:s0"
        attr="bikes"
        views={{ number: (shown) => <p>{`my chart of ${shown.length}`}</p> }}
      />,
    );
    expect(await screen.findByText("my chart of 2")).toBeInTheDocument();
  });
});

describe("the edit parts", () => {
  const editing = config({ mode: "edit", editableAttrs: ["bikes"] });
  const writable = () => ({ ...fixtureSource(entities), patch: vi.fn(async () => undefined) }) as never;

  it("GridPending counts what is typed and GridReview applies it through the source", async () => {
    const source = writable() as { patch: ReturnType<typeof vi.fn> };
    render(
      <Host
        config={editing}
        source={source as never}
        parts={(view) => (
          <>
            <GridPending view={view} />
            <GridTable view={view} />
            <GridReview view={view} />
          </>
        )}
      />,
    );
    await rows();
    fireEvent.change(screen.getAllByLabelText(`${DEFAULT_LABELS.edit} Bikes`)[0], { target: { value: "9" } });
    fireEvent.click(await screen.findByRole("button", { name: DEFAULT_LABELS.review }));
    const review = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    fireEvent.click(within(review).getByRole("button", { name: DEFAULT_LABELS.apply }));
    await waitFor(() => expect(source.patch).toHaveBeenCalled());
  });

  it("GridNotices shows the pending list, and Discard drops it", async () => {
    render(<Host config={editing} source={writable()} parts={(view) => <><GridNotices view={view} /><GridTable view={view} /></>} />);
    await rows();
    fireEvent.change(screen.getAllByLabelText(`${DEFAULT_LABELS.edit} Bikes`)[0], { target: { value: "9" } });
    expect(await screen.findByRole("button", { name: DEFAULT_LABELS.discard })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.discard }));
    await waitFor(() => expect(screen.queryByRole("button", { name: DEFAULT_LABELS.discard })).toBeNull());
  });
});

describe("the states around the rows", () => {
  it("GridStatus says empty in the host's words, and a failed read", async () => {
    render(<Host source={fixtureSource([])} empty="Nothing yet" parts={(view) => <GridStatus view={view} />} />);
    expect(await screen.findByText("Nothing yet")).toBeInTheDocument();
    cleanup();
    const failing = { ...fixtureSource([]), query: async () => { throw new Error("no answer"); } };
    render(<Host source={failing} parts={(view) => <GridStatus view={view} />} />);
    expect(await screen.findByText(`${DEFAULT_LABELS.error}: no answer`, { exact: false })).toBeInTheDocument();
  });

  it("GridFrame draws the refusal in place of every part", async () => {
    const refused = { ...fixtureSource([]), query: async () => { throw new SourceError(401, "sign in"); } };
    render(<Host source={refused} onSignIn={() => undefined} parts={(view) => <GridTable view={view} />} />);
    expect(await screen.findByRole("heading", { name: DEFAULT_LABELS.signedOutTitle })).toBeInTheDocument();
    expect(screen.queryByRole("grid")).toBeNull();
  });

  it("HiddenFields lists what is hidden and gives it back", () => {
    const onShow = vi.fn();
    render(<HiddenFields attrs={["a", "b"]} labels={DEFAULT_LABELS} labelOf={(attr) => attr.toUpperCase()} onShow={onShow} />);
    fireEvent.click(screen.getByRole("button", { name: `${DEFAULT_LABELS.hiddenFields} (2)` }));
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.showAllColumns }));
    expect(onShow).toHaveBeenCalledWith(["a", "b"]);
  });
});

describe("a host's own shape", () => {
  beforeDialog();

  it("table only: no other part draws or reads", async () => {
    const source = { ...fixtureSource(entities), tilesAt: vi.fn() };
    render(<Host source={source as never} parts={(view) => <GridTable view={view} />} />);
    await rows();
    expect(document.querySelector(".jc-grid-pager, .jc-grid-query, .jc-grid-map")).toBeNull();
  });

  it("table + pager: a page turned moves the rows", async () => {
    render(<Host parts={(view) => <><GridTable view={view} /><GridPager view={view} /></>} />);
    await rows();
    fireEvent.click(screen.getByRole("button", { name: new RegExp(DEFAULT_LABELS.next) }));
    expect(await screen.findByRole("button", { name: /Station 2/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Station 0/ })).toBeNull();
  });

  it("table + map: a row and its shape are on one page", async () => {
    render(<Host config={config({ map: { enabled: true, attr: "location" } })} parts={(view) => <><GridTable view={view} /><GridMapPanel view={view} /></>} />);
    await rows();
    expect(await screen.findByRole("button", { name: /^shape urn:ngsi-ld:Station:s0/ })).toBeInTheDocument();
  });

  it("detail only: an id opens its row with no table on the page", async () => {
    render(<Host openRow="urn:ngsi-ld:Station:s2" parts={(view) => <><GridDetail view={view} /><GridCellPanels view={view} /></>} />);
    expect(await screen.findByRole("heading", { name: /Station 2/, hidden: true })).toBeInTheDocument();
    expect(screen.queryByRole("grid")).toBeNull();
  });

  it("EntityGrid keeps drawing every part from the same props", async () => {
    render(<EntityGrid config={config()} source={fixtureSource(entities)} />);
    await rows();
    expect(screen.getByRole("grid")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: new RegExp(DEFAULT_LABELS.next) })).toBeInTheDocument();
    expect(screen.getByText(DEFAULT_LABELS.query)).toBeInTheDocument();
  });
});

/** jsdom has no modal dialog: `showModal` opens it as the browser would. */
function beforeDialog(): void {
  HTMLDialogElement.prototype.showModal ??= function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close ??= function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
}
