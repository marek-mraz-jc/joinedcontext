/**
 * A value with history the visitor may read wears a mark (T-6878): the column header where the
 * Endpoint's `access/attributes` says `history: true` (one read per grid), a cell where the row's own
 * answer shows the value changed (no request per cell). The mark opens the history; a public
 * reader's versions show values and times, never who changed a value.
 */
import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { EntityHistory } from "../src/grid/EntityHistory";
import { parseGridConfig } from "../src/grid/config";
import { accessOf, endpointSource } from "../src/grid/source";
import type { EntitySource } from "../src/grid/source";
import { DEFAULT_LABELS } from "../src/grid/useEntityGrid";
import type { JcRequest, JcResponse } from "../src/sdk/transport";

const TYPE = "AirQualityObserved";
const ID = `urn:ngsi-ld:${TYPE}:bb:station-1`;
const entities = [
  {
    id: ID,
    type: TYPE,
    name: { type: "Property", value: "Námestie SNP" },
    pm10: { type: "Property", value: 31, observedAt: "2026-11-14T10:00:00Z" },
    stationName: { type: "Property", value: "SNP" },
  },
];

function configOf() {
  const parsed = parseGridConfig({
    source: { kind: "fixture", name: "test" },
    type: TYPE,
    columns: [
      { attr: "name", label: "Name" },
      { attr: "pm10", label: "PM10" },
      { attr: "stationName", label: "Station" },
    ],
    pageSize: 10,
    history: { enabled: true },
  });
  expect(parsed.findings, JSON.stringify(parsed.findings)).toEqual([]);
  return { ...parsed.config!, mode: "view" as const };
}

/** An Endpoint answering a page, the access table and a temporal read, recording every path. */
function endpoint(history: Record<string, boolean | undefined>) {
  const paths: string[] = [];
  const transport = async (request: JcRequest): Promise<JcResponse> => {
    paths.push(request.path);
    if (request.path.includes("/access/attributes")) {
      const attributes = ["name", "pm10", "stationName"].map((name) => ({
        name,
        read: true,
        write: false,
        delete: false,
        ...(history[name] === undefined ? {} : { history: history[name] }),
        reason: "grant",
      }));
      return { status: 200, body: { type: TYPE, attributes }, headers: {} };
    }
    if (request.path.includes("/temporal/entities/")) {
      return {
        status: 200,
        body: { id: ID, type: TYPE, pm10: { type: "Property", values: [[28, "2026-11-14T08:00:00Z"], [31, "2026-11-14T10:00:00Z"]] } },
        headers: {},
      };
    }
    return { status: 200, body: entities, headers: { "ngsild-results-count": "1" } };
  };
  return { paths, source: endpointSource("air", transport) };
}

const temporal = (paths: string[]) => paths.filter((path) => path.includes("/temporal/"));

describe("the history mark (T-6878)", () => {
  it("reads history from the access answer, deny by default", () => {
    const rows = accessOf({ attributes: [
      { name: "a", read: true, history: true },
      { name: "b", read: true, history: "yes" },
      { name: "c", read: true },
    ] });
    expect(rows.map((row) => row.history)).toEqual([true, false, undefined]);
  });

  it("a_column_with_public_temporal_read_shows_the_history_icon", async () => {
    const { paths, source } = endpoint({ name: false, pm10: true, stationName: false });
    render(<EntityGrid config={configOf()} source={source} publicView />);
    await screen.findByText("Námestie SNP");
    const mark = await screen.findByRole("button", { name: `${DEFAULT_LABELS.historyAvailable}: PM10` });
    expect(mark.closest("th")).not.toBeNull();
    expect(screen.queryByRole("button", { name: `${DEFAULT_LABELS.historyAvailable}: Station` })).toBeNull();
    // One page read and one capability read: the mark itself asks nothing.
    expect(paths.filter((path) => path.includes("/access/attributes"))).toHaveLength(1);
    expect(temporal(paths)).toEqual([]);
    fireEvent.click(mark);
    await screen.findByRole("img", { name: /PM10/ });
    expect(temporal(paths)).toHaveLength(1);
    expect(temporal(paths)[0]).toContain(`/temporal/entities/${encodeURIComponent(ID)}?attrs=pm10`);
  });

  it("a_cell_whose_value_changed_shows_the_icon_without_a_request", async () => {
    const { paths, source } = endpoint({ name: true, pm10: true, stationName: true });
    render(<EntityGrid config={configOf()} source={source} publicView />);
    await screen.findByText("Námestie SNP");
    const cell = await screen.findByRole("button", { name: `${DEFAULT_LABELS.historyOfValue}: PM10` });
    expect(cell.closest("td")).not.toBeNull();
    // A value with no observedAt and no change after its creation wears none.
    expect(screen.queryByRole("button", { name: `${DEFAULT_LABELS.historyOfValue}: Station` })).toBeNull();
    expect(temporal(paths)).toEqual([]);
    const before = paths.length;
    fireEvent.click(cell);
    await waitFor(() => expect(temporal(paths)).toHaveLength(1));
    expect(paths.length).toBe(before + 1);
    // The keyboard opens it too: `h` on the active cell.
    const grid = screen.getByRole("grid");
    grid.focus();
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.keyDown(grid, { key: "h" });
    await screen.findByRole("region", { name: /PM10/ });
  });

  it("ctrl_cmd_or_alt_h_on_a_marked_cell_stays_the_browsers", async () => {
    const { paths, source } = endpoint({ name: true, pm10: true, stationName: true });
    render(<EntityGrid config={configOf()} source={source} publicView />);
    await screen.findByRole("button", { name: `${DEFAULT_LABELS.historyOfValue}: PM10` });
    const grid = screen.getByRole("grid");
    grid.focus();
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    for (const modifier of [{ ctrlKey: true }, { metaKey: true }, { altKey: true }]) {
      // The grid leaves the event alone, so the browser's own Ctrl/Cmd+H (history) still runs.
      expect(fireEvent.keyDown(grid, { key: "h", ...modifier })).toBe(true);
    }
    expect(screen.queryByRole("region", { name: /PM10/ })).toBeNull();
    expect(temporal(paths)).toEqual([]);
  });

  it("no_temporal_grant_no_icon_no_request", async () => {
    const { paths, source } = endpoint({ name: false, pm10: false, stationName: false });
    render(<EntityGrid config={configOf()} source={source} publicView />);
    await screen.findByText("Námestie SNP");
    await waitFor(() => expect(paths.some((path) => path.includes("/access/attributes"))).toBe(true));
    expect(document.querySelector(".jc-grid-history-mark")).toBeNull();
    const grid = screen.getByRole("grid");
    grid.focus();
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.keyDown(grid, { key: "h" });
    // Nor does the column menu offer a history the Policy refuses.
    fireEvent.click(screen.getByRole("button", { name: `${DEFAULT_LABELS.columnMenu}: PM10` }));
    expect(within(screen.getByRole("menu")).queryByRole("menuitem", { name: new RegExp(DEFAULT_LABELS.history) })).toBeNull();
    expect(temporal(paths)).toEqual([]);
  });

  it("an older gateway without the member draws no mark and keeps the menu", async () => {
    const { source } = endpoint({});
    render(<EntityGrid config={configOf()} source={source} publicView />);
    await screen.findByText("Námestie SNP");
    expect(document.querySelector(".jc-grid-history-mark")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: `${DEFAULT_LABELS.columnMenu}: PM10` }));
    expect(within(screen.getByRole("menu")).getByRole("menuitem", { name: new RegExp(DEFAULT_LABELS.history) })).toBeInTheDocument();
  });

  it("public_versions_carry_no_person_provenance", async () => {
    // Even were a server to send a writer, a public reader's table has no column for it.
    const source: EntitySource = {
      query: async () => ({ rows: [] }),
      get: async () => null,
      history: async (_id, _attr, window) =>
        window.versions
          ? [
              { at: "2026-11-14T08:00:00Z", value: 28, supersededAt: "2026-11-14T08:05:00Z", datasetId: "urn:ngsi-ld:Dataset:raw", provenance: { actor: "user:jana", reason: "Jana fixed it", channel: "ui" } },
              { at: "2026-11-14T08:00:00Z", value: 27, datasetId: "urn:ngsi-ld:Dataset:raw", provenance: { channel: "ui" } },
            ]
          : [{ at: "2026-11-14T08:00:00Z", value: 27 }],
    };
    render(<EntityHistory source={source} id={ID} attr="pm10" versions versionsFirst publicView />);
    const table = await screen.findByRole("table");
    expect(within(table).getAllByText("urn:ngsi-ld:Dataset:raw")).toHaveLength(2);
    expect(within(table).getByText("28")).toBeInTheDocument();
    expect(table.textContent).not.toMatch(/jana|Changed by|Reason/i);
    expect(screen.getByRole("checkbox", { name: "Every version of each value" })).toBeChecked();
  });

  it("a signed-in reader keeps who changed it", async () => {
    const source: EntitySource = {
      query: async () => ({ rows: [] }),
      get: async () => null,
      history: async () => [{ at: "2026-11-14T08:00:00Z", value: 28, supersededAt: "2026-11-14T08:05:00Z", provenance: { actor: "user:jana", channel: "ui" } }],
    };
    render(<EntityHistory source={source} id={ID} attr="pm10" versions versionsFirst />);
    const table = await screen.findByRole("table");
    expect(table.textContent).toContain("user:jana");
  });

  it("a public reader refused the versions sees no switch and no word on rights", async () => {
    const { SourceError } = await import("../src/grid/source");
    const source: EntitySource = {
      query: async () => ({ rows: [] }),
      get: async () => null,
      history: async (_id, _attr, window) => {
        if (window.versions) throw new SourceError(403, "versions=all needs readVersions");
        return [{ at: "2026-11-14T08:00:00Z", value: 27 }];
      },
    };
    render(<EntityHistory source={source} id="urn:ngsi-ld:X:refused" attr="pm10" versions versionsFirst publicView />);
    await screen.findByRole("table");
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(document.body.textContent).not.toMatch(/role|readVersions/i);
  });
});
