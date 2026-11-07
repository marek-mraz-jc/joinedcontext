// T-4828: a row opens as a modal dialog with Fields, History and the host's tabs, can be named by
// the host (a deep link), and asks the gateway for every version only when the person asks.
import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, fireEvent, within } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { RowDetail } from "../src/grid/RowDetail";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource, historyOf, SourceError } from "../src/grid/source";
import type { EntitySource, HistoryPoint, HistoryWindow } from "../src/grid/source";

const KPI = "urn:ngsi-ld:KeyPerformanceIndicator:kpi-1";
const OTHER = "urn:ngsi-ld:KeyPerformanceIndicator:kpi-2";
const kpis: Record<string, unknown>[] = [
  {
    id: KPI,
    type: "KeyPerformanceIndicator",
    name: { type: "Property", value: "Recycling rate" },
    kpiValue: { type: "Property", value: 41.5, observedAt: "2026-10-20T10:00:00Z" },
  },
  {
    id: OTHER,
    type: "KeyPerformanceIndicator",
    name: { type: "Property", value: "Bus punctuality" },
    kpiValue: { type: "Property", value: 88, observedAt: "2026-10-20T10:00:00Z" },
  },
];

function configOf(mode: "edit" | "view", pageSize = 10) {
  return parseGridConfig({
    source: { kind: "fixture", name: "test" },
    type: "KeyPerformanceIndicator",
    columns: [
      { attr: "name", label: "Name", pinned: true },
      { attr: "kpiValue", label: "Value" },
    ],
    pageSize,
    mode,
    history: { enabled: true },
    ...(mode === "edit" ? { editableAttrs: ["kpiValue"] } : {}),
  }).config!;
}

type History = (id: string, attr: string, window: HistoryWindow) => Promise<HistoryPoint[]>;

function sourceWith(history?: History): EntitySource {
  const base = fixtureSource(kpis);
  return { ...base, patch: vi.fn(async () => undefined), ...(history ? { history } : {}) };
}

describe("row dialog (T-4828)", () => {
  it("opens as a modal with Fields first, moves between tabs by arrow keys and shows the host's tab", async () => {
    const history = vi.fn<History>(async () => []);
    render(
      <EntityGrid
        config={configOf("edit")}
        source={sourceWith(history)}
        detailTabs={(row) => [{ id: "comments", label: "Comments", content: <p>{`comments of ${row.id}`}</p> }]}
      />,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Open: Recycling rate" }));
    const dialog = screen.getByRole("dialog", { name: "Details: Recycling rate" });
    const tabs = within(dialog).getAllByRole("tab");
    expect(tabs.map((tab) => tab.textContent)).toEqual(["Fields", "History", "Metadata", "Comments"]);
    expect(tabs[0]).toHaveAttribute("aria-selected", "true");
    // Fields edit with the grid's own editor and join its pending list, shown inside the dialog.
    fireEvent.change(within(dialog).getByRole("spinbutton"), { target: { value: "42" } });
    expect(within(dialog).getByText("1 not applied yet")).toBeInTheDocument();

    fireEvent.keyDown(tabs[0], { key: "ArrowRight" });
    expect(within(dialog).getByRole("tab", { name: "History" })).toHaveAttribute("aria-selected", "true");
    expect(within(dialog).getByRole("tab", { name: "History" })).toHaveFocus();
    // History opens on the measured attribute, not on the name that does not change (T-5602).
    await waitFor(() => expect(history).toHaveBeenCalledWith(KPI, "kpiValue", expect.objectContaining({ lastN: expect.any(Number) })));
    expect(history).not.toHaveBeenCalledWith(KPI, "name", expect.anything());
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Attribute" }), { target: { value: "name" } });
    await waitFor(() => expect(history).toHaveBeenLastCalledWith(KPI, "name", expect.anything()));

    fireEvent.keyDown(within(dialog).getByRole("tab", { name: "History" }), { key: "End" });
    expect(within(dialog).getByRole("tabpanel")).toHaveTextContent(`comments of ${KPI}`);
  });

  it("opens on the tab and attribute a host names and reports each change (T-5602)", async () => {
    const history = vi.fn<History>(async () => []);
    const onPlace = vi.fn();
    render(
      <EntityGrid
        config={configOf("view")}
        source={sourceWith(history)}
        openRow={KPI}
        detailPlace={{ tab: "history", attr: "name" }}
        onDetailPlace={onPlace}
      />,
    );
    const dialog = await screen.findByRole("dialog", { name: "Details: Recycling rate" });
    expect(within(dialog).getByRole("tab", { name: "History" })).toHaveAttribute("aria-selected", "true");
    await waitFor(() => expect(history).toHaveBeenCalledWith(KPI, "name", expect.anything()));
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Attribute" }), { target: { value: "kpiValue" } });
    expect(onPlace).toHaveBeenLastCalledWith({ tab: "history", attr: "kpiValue" });
    fireEvent.click(within(dialog).getByRole("tab", { name: "Fields" }));
    expect(onPlace).toHaveBeenLastCalledWith({ tab: "fields", attr: "kpiValue" });
  });

  it("reads a tab or attribute the row does not have as the defaults (T-5602)", async () => {
    const history = vi.fn<History>(async () => []);
    render(<EntityGrid config={configOf("view")} source={sourceWith(history)} openRow={KPI} detailPlace={{ tab: "model", attr: "<script>" }} />);
    const dialog = await screen.findByRole("dialog", { name: "Details: Recycling rate" });
    expect(within(dialog).getByRole("tab", { name: "Fields" })).toHaveAttribute("aria-selected", "true");
    fireEvent.click(within(dialog).getByRole("tab", { name: "History" }));
    await waitFor(() => expect(history).toHaveBeenCalledWith(KPI, "kpiValue", expect.anything()));
  });

  it("has no History tab where the source reads no history", async () => {
    render(<EntityGrid config={configOf("edit")} source={sourceWith()} />);
    fireEvent.click(await screen.findByRole("button", { name: "Open: Recycling rate" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["Fields", "Metadata"]);
  });

  it("shows values without editors to a person the grant gives no write", async () => {
    render(<EntityGrid config={configOf("view")} source={sourceWith()} />);
    fireEvent.click(await screen.findByRole("button", { name: "Open: Recycling rate" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).queryByRole("spinbutton")).toBeNull();
    expect(within(dialog).queryByRole("textbox")).toBeNull();
    expect(dialog).toHaveTextContent("41.5");
  });

  it("opens the row a host names, reads one off the page by its id, and reports the close", async () => {
    const source = sourceWith();
    const get = vi.spyOn(source, "get");
    const onOpenRow = vi.fn();
    // One row a page: the second KPI is not loaded, so the dialog reads it by its id.
    render(<EntityGrid config={configOf("edit", 1)} source={source} openRow={OTHER} onOpenRow={onOpenRow} />);
    const dialog = await screen.findByRole("dialog", { name: "Details: Bus punctuality" });
    expect(get).toHaveBeenCalledWith(OTHER);
    // A row read on its own has no editors: an edit is written against the loaded page.
    expect(within(dialog).queryByRole("spinbutton")).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(onOpenRow).toHaveBeenLastCalledWith(null);
  });

  // T-5600: a row read by its id showed "6.7" without its unit, an ISO time and "Point".
  it("shows a row read by its id as the loaded page shows it", async () => {
    const measured = kpis.map((kpi) => ({
      ...kpi,
      kpiValue: { ...(kpi.kpiValue as object), unitCode: "GQ" },
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.94, 60.17] } },
    }));
    const config = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "KeyPerformanceIndicator",
      columns: [{ attr: "name", label: "Name", pinned: true }, { attr: "kpiValue", label: "Value" }, { attr: "location", label: "Place" }],
      pageSize: 10,
    }).config!;
    const fields = async (pageSize: number) => {
      const { unmount } = render(<EntityGrid config={{ ...config, pageSize }} source={fixtureSource(measured)} openRow={OTHER} onOpenRow={vi.fn()} />);
      const text = (await screen.findByRole("dialog", { name: "Details: Bus punctuality" })).textContent;
      unmount();
      return text;
    };
    const listed = await fields(10);
    expect(listed).toContain("µg/m³");
    expect(await fields(1)).toBe(listed);
  });

  it("closes a link to a row that is gone or not readable", async () => {
    const onOpenRow = vi.fn();
    render(<EntityGrid config={configOf("edit")} source={sourceWith()} openRow="urn:ngsi-ld:KeyPerformanceIndicator:none" onOpenRow={onOpenRow} />);
    await waitFor(() => expect(onOpenRow).toHaveBeenCalledWith(null));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("asks for every version only when switched on and says a refusal in words", async () => {
    const history = vi.fn<History>(async (_id, _attr, window) => {
      if (window.versions) throw new SourceError(403, "readVersions not granted");
      return [{ at: "2026-10-20T10:00:00Z", value: 41.5 }];
    });
    render(<EntityGrid config={configOf("edit")} source={sourceWith(history)} />);
    fireEvent.click(await screen.findByRole("button", { name: "Open: Recycling rate" }));
    fireEvent.click(screen.getByRole("tab", { name: "History" }));
    await waitFor(() => expect(history).toHaveBeenCalled());
    expect(history.mock.calls.every(([, , window]) => !window.versions)).toBe(true);
    fireEvent.click(screen.getByRole("checkbox", { name: /Every version/ }));
    expect(await screen.findByText(/Your role may not read earlier versions/)).toBeInTheDocument();
    expect(history).toHaveBeenCalledWith(KPI, "kpiValue", expect.objectContaining({ versions: true }));
    // Refused once, the switch turns off and the readings the role may read come back.
    expect(screen.getByRole("checkbox", { name: /Every version/ })).toBeDisabled();
    await waitFor(() => expect(history.mock.lastCall?.[2].versions).toBeFalsy());
  });

  it("lists each version with who changed it and why, markup as text", async () => {
    const history = vi.fn<History>(async () => [
      {
        at: "2026-10-20T10:00:00Z",
        value: 40,
        supersededAt: "2026-10-20T11:00:00Z",
        provenance: { channel: "pipeline", operation: "create", actor: "pipeline-kpi" },
      },
      {
        at: "2026-10-20T10:00:00Z",
        value: 41.5,
        provenance: { channel: "ui", operation: "correct", reason: "<img src=x onerror=alert(1)>checked" },
      },
    ]);
    render(<EntityGrid config={configOf("edit")} source={sourceWith(history)} />);
    fireEvent.click(await screen.findByRole("button", { name: "Open: Recycling rate" }));
    fireEvent.click(screen.getByRole("tab", { name: "History" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /Every version/ }));
    const dialog = screen.getByRole("dialog");
    expect(await within(dialog).findByText("pipeline · create · pipeline-kpi")).toBeInTheDocument();
    expect(within(dialog).getByText("ui · correct")).toBeInTheDocument();
    expect(within(dialog).getByText("<img src=x onerror=alert(1)>checked")).toBeInTheDocument();
    expect(dialog.querySelector("img")).toBeNull();
    expect(within(dialog).getByRole("columnheader", { name: "Reason" })).toBeInTheDocument();
  });
});

// T-5599: the detail names no URN (the id is behind a copy button), reads an enum by its label and
// says "not set" for an attribute with no value, on the page and read on its own.
describe("row dialog values (T-5599)", () => {
  const EVENT = "urn:ngsi-ld:Event:hel.fi:helsinki:elo-1";
  const LATER = "urn:ngsi-ld:Event:hel.fi:helsinki:elo-2";
  const events: Record<string, unknown>[] = [EVENT, LATER].map((id) => ({
    id,
    type: "Event",
    eventStatus: { type: "Property", value: "EventScheduled" },
    address: { type: "Property", value: {} },
  }));
  const config = parseGridConfig({
    source: { kind: "fixture", name: "test" },
    type: "Event",
    columns: [
      { attr: "eventStatus", label: "Status" },
      { attr: "address", label: "Address" },
    ],
    pageSize: 1,
    mode: "view",
  }).config!;
  const enums = { eventStatus: [{ value: "EventScheduled", title: "Scheduled" }] };

  it.each([
    ["on the page", undefined, EVENT],
    ["read on its own", LATER, LATER],
  ])("shows no URN, the enum label and not set %s, and copies the id", async (_, openRow, id) => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    render(<EntityGrid config={config} source={fixtureSource(events)} enums={enums} openRow={openRow} />);
    if (!openRow) fireEvent.click(await screen.findByRole("button", { name: `Open: ${EVENT.split(":").at(-1)}` }));
    const dialog = await screen.findByRole("dialog", { name: `Details: ${id.split(":").at(-1)}` });
    expect(dialog.textContent).not.toMatch(/urn:/);
    const status = within(dialog).getByText("Status").nextElementSibling;
    expect(status).toHaveTextContent(/^Scheduled$/);
    expect(within(dialog).getByText("Address").nextElementSibling).toHaveTextContent(/^not set$/);
    fireEvent.click(within(dialog).getByRole("button", { name: "Copy identifier" }));
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Copied" })).toBeInTheDocument());
    expect(writeText).toHaveBeenCalledWith(id);
  });
});

describe("versions in a temporal answer (DM-114)", () => {
  it("keeps supersededAt and the text fields of provenance, organization and actorKind included, nothing else", () => {
    const body = {
      kpiValue: [
        {
          type: "Property",
          value: 40,
          observedAt: "2026-10-20T10:00:00Z",
          supersededAt: "2026-10-20T11:00:00Z",
          provenance: { channel: "ui", actor: "a1", operation: "correct", reason: "typo", requestId: "9a07", writtenAt: "2026-10-20T11:00:00Z", organization: "helsinki", actorKind: "person", extra: { nested: 1 } },
        },
        { type: "Property", value: 41, observedAt: "2026-10-20T10:00:00Z" },
      ],
    };
    expect(historyOf(body, "kpiValue")).toEqual([
      {
        at: "2026-10-20T10:00:00Z",
        value: 40,
        supersededAt: "2026-10-20T11:00:00Z",
        provenance: { channel: "ui", actor: "a1", operation: "correct", reason: "typo", writtenAt: "2026-10-20T11:00:00Z", organization: "helsinki", actorKind: "person" },
      },
      { at: "2026-10-20T10:00:00Z", value: 41 },
    ]);
  });
});

// T-5619: the host gives the focus back to the grid on close, which a still-modal dialog makes inert
// in a browser; the dialog leaves its modal state before it tells the host.
describe("closing the row dialog (T-5619)", () => {
  it.each(["Escape", "Close"])("is out of its modal state when the host hears of %s", async (how) => {
    const row = { id: "urn:ngsi-ld:Event:hel.fi:helsinki:elo-1", type: "Event", cells: {}, raw: {} };
    let openAtClose: boolean | null = null;
    render(
      <RowDetail
        row={row}
        name="Elo"
        columns={[]}
        renderValue={() => null}
        labels={{ rowDetail: "Details", close: "Close", rowFields: "Fields", rowHistory: "History", historyAttr: "Attribute" }}
        onClose={() => {
          openAtClose = document.querySelector("dialog")?.hasAttribute("open") ?? null;
        }}
      />,
    );
    const dialog = await screen.findByRole("dialog", { name: "Details: Elo" });
    if (how === "Escape") fireEvent.keyDown(dialog, { key: "Escape" });
    else fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(openAtClose).toBe(false);
  });
});
