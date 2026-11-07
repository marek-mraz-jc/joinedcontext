/**
 * The permission-aware grid (T-5530): what the person cannot change wears a lock with the reason,
 * what they may not read stands as a "hidden: no access" column and is never asked for, the review
 * reads names and formatted values, and a paste into a locked cell is counted as not allowed.
 * Every decision is the server's `access/attributes` answer; the grid only renders it.
 */
import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource } from "../src/grid/source";
import type { AttributeAccess, GridQuery } from "../src/grid/source";
import { DEFAULT_LABELS } from "../src/grid/useEntityGrid";
import { DEFAULT_INSPECTOR_LABELS } from "../src/grid/CellInspector";

const TYPE = "BikeHireDockingStation";
const ID = `urn:ngsi-ld:${TYPE}:hel:helsinki:001`;
const POI = "urn:ngsi-ld:PointOfInterest:hel:poi:kamppi";
const entities: Record<string, unknown>[] = [
  {
    id: ID,
    type: TYPE,
    name: { type: "Property", value: "Kamppi" },
    availableBikeNumber: { type: "Property", value: 5 },
    status: { type: "Property", value: "working" },
    totalSlotNumber: { type: "Property", value: 15 },
    capacity: { type: "Property", value: 20 },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.93, 60.17] } },
    refPointOfInterest: { type: "Relationship", object: POI },
    operatorNote: { type: "Property", value: "secret note" },
  },
];

const row = (name: string, read: boolean, write: boolean, reason: AttributeAccess["reason"]): AttributeAccess => ({ name, read, write, delete: false, reason });
const ACCESS: AttributeAccess[] = [
  row("name", true, true, "grant"),
  row("availableBikeNumber", true, false, "not_in_grant"),
  row("status", true, false, "prohibited"),
  row("totalSlotNumber", true, false, "formula"),
  row("capacity", true, false, "grant"),
  row("location", true, true, "grant"),
  row("refPointOfInterest", true, true, "grant"),
  row("operatorNote", false, false, "not_in_grant"),
];

const ATTRS = ["name", "availableBikeNumber", "status", "totalSlotNumber", "capacity", "location", "refPointOfInterest", "operatorNote"];

function configOf() {
  const parsed = parseGridConfig({
    source: { kind: "fixture", name: "test" },
    type: TYPE,
    columns: ATTRS.map((attr) => ({ attr, label: attr === "name" ? "Name" : attr === "location" ? "Location" : attr === "refPointOfInterest" ? "Point of interest" : attr })),
    mode: "edit",
    editableAttrs: ["name", "location", "refPointOfInterest", "availableBikeNumber"],
    filters: { preset: { attrs: ATTRS } },
    pageSize: 10,
  });
  expect(parsed.findings, JSON.stringify(parsed.findings)).toEqual([]);
  return parsed.config!;
}

/** The fixture source, recording every query's `attrs`. */
function recorded() {
  const inner = fixtureSource(entities);
  const asked: (string[] | undefined)[] = [];
  return {
    asked,
    source: {
      ...inner,
      query: async (...args: Parameters<typeof inner.query>) => {
        asked.push((args[0] as GridQuery | undefined)?.attrs);
        return inner.query(...args);
      },
      patch: vi.fn(async () => undefined),
    },
  };
}

describe("the permission-aware grid", () => {
  it("puts the keyboard into the cell's editor, not the cell's info button before it (T-6230)", async () => {
    const { source } = recorded();
    render(<EntityGrid config={configOf()} source={source} access={ACCESS} />);
    const grid = screen.getByRole("grid");
    const kamppi = await screen.findByDisplayValue("Kamppi");
    expect(kamppi.closest("td")?.querySelector(".jc-grid-info")).not.toBeNull();
    act(() => kamppi.focus());
    fireEvent.keyDown(kamppi, { key: "Escape" });
    expect(grid).toHaveFocus();
    for (const key of ["F2", "Enter"]) {
      fireEvent.keyDown(grid, { key });
      expect(kamppi).toHaveFocus();
      fireEvent.keyDown(kamppi, { key: "Escape" });
    }
  });

  it("marks each column the person cannot change with a lock named by its reason", async () => {
    const { source } = recorded();
    render(<EntityGrid config={configOf()} source={source} access={ACCESS} />);
    await screen.findByDisplayValue("Kamppi");
    const lock = (attr: string) => screen.getByRole("columnheader", { name: new RegExp(`^.*${attr}`) }).querySelector(".jc-grid-lock");
    expect(screen.getByRole("img", { name: `${DEFAULT_LABELS.locked}: ${DEFAULT_INSPECTOR_LABELS.notInGrant}` })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: `${DEFAULT_LABELS.locked}: ${DEFAULT_INSPECTOR_LABELS.prohibited}` })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: `${DEFAULT_LABELS.locked}: ${DEFAULT_INSPECTOR_LABELS.formula}` })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: `${DEFAULT_LABELS.locked}: ${DEFAULT_LABELS.lockReadOnly}` })).toBeInTheDocument();
    expect(lock("availableBikeNumber")).toHaveAttribute("title", DEFAULT_INSPECTOR_LABELS.notInGrant);
    // A column the person may change wears none, and its cell opens an editor.
    expect(screen.getByDisplayValue("Kamppi").closest("td")).not.toHaveClass("jc-grid-td--locked");
    // A locked cell is muted, its value still read.
    expect(screen.getByText("working").closest("td")).toHaveClass("jc-grid-td--locked");
  });

  it("says why when the server gives no row for an attribute and when the table does not offer it", async () => {
    const { source } = recorded();
    const access = ACCESS.filter((one) => one.name !== "capacity").map((one) => (one.name === "totalSlotNumber" ? { ...one, write: true, reason: "grant" as const } : one));
    render(<EntityGrid config={configOf()} source={source} access={access} />);
    await screen.findByDisplayValue("Kamppi");
    expect(screen.getByRole("img", { name: `${DEFAULT_LABELS.locked}: ${DEFAULT_INSPECTOR_LABELS.noGrant}` })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: `${DEFAULT_LABELS.locked}: ${DEFAULT_LABELS.lockNotOffered}` })).toBeInTheDocument();
  });

  it("draws no lock before the decisions are read or where nobody asks for them", async () => {
    const { source } = recorded();
    render(<EntityGrid config={configOf()} source={source} />);
    await screen.findByDisplayValue("Kamppi");
    expect(screen.queryByRole("img", { name: new RegExp(`^${DEFAULT_LABELS.locked}`) })).toBeNull();
  });

  it("leaves an unreadable attribute out of the grid altogether and never asks for it (T-6210)", async () => {
    const { source, asked } = recorded();
    render(<EntityGrid config={configOf()} source={source} access={ACCESS} />);
    await screen.findByDisplayValue("Kamppi");
    // No column, no mark, no hatched cell: what the person may not read is not drawn at all.
    expect(screen.queryByRole("columnheader", { name: /operatorNote/ })).toBeNull();
    expect(document.body.textContent).not.toContain("operatorNote");
    expect(document.body.textContent).not.toContain("secret note");
    expect(asked.length).toBeGreaterThan(0);
    for (const attrs of asked) expect(attrs ?? []).not.toContain("operatorNote");
  });

  it("says how many fields the person's access leaves out, without naming them (T-6210)", async () => {
    const { source } = recorded();
    render(<EntityGrid config={configOf()} source={source} access={ACCESS} />);
    await screen.findByDisplayValue("Kamppi");
    expect(screen.getByText(DEFAULT_LABELS.notReadable.replace("{count}", "1"))).toBeTruthy();
  });

  it("says nothing about missing fields when the person reads them all", async () => {
    const { source } = recorded();
    render(<EntityGrid config={configOf()} source={source} />);
    await screen.findByDisplayValue("Kamppi");
    expect(document.querySelector(".jc-grid-not-readable")).toBeNull();
  });

  it("draws no lock on a read-only grid", async () => {
    const { source } = recorded();
    render(<EntityGrid config={{ ...configOf(), mode: "view" }} source={source} access={ACCESS} />);
    await screen.findByText("Kamppi");
    expect(screen.queryByRole("img", { name: new RegExp(`^${DEFAULT_LABELS.locked}`) })).toBeNull();
  });

  it("reviews a GeoProperty and a Relationship change by name, label and formatted value", async () => {
    const { source } = recorded();
    render(
      <EntityGrid
        config={configOf()}
        source={source}
        access={ACCESS}
        state={{ edits: { [ID]: { location: { type: "Point", coordinates: [24.95, 60.18] }, refPointOfInterest: "urn:ngsi-ld:PointOfInterest:hel:poi:kallio" } } }}
      />,
    );
    await screen.findByDisplayValue("Kamppi");
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    const text = panel.textContent ?? "";
    expect(text).not.toContain("urn:");
    expect(text).not.toContain("[object Object]");
    const geo = within(panel).getByRole("row", { name: /Location/ });
    expect(geo.textContent).toContain("Kamppi");
    expect(geo.textContent).toMatch(/60\.18/);
    const rel = within(panel).getByRole("row", { name: /Point of interest/ });
    expect(rel.textContent).toContain("kamppi");
    expect(rel.textContent).toContain("kallio");
    // The identifier is one click away, not printed.
    const writeText = vi.fn(async () => undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    fireEvent.click(within(geo).getByRole("button", { name: `${DEFAULT_LABELS.copyId}: Kamppi` }));
    expect(writeText).toHaveBeenCalledWith(ID);
  });

  it("says why when a person types on a locked cell", async () => {
    const { source } = recorded();
    render(<EntityGrid config={configOf()} source={source} access={ACCESS} />);
    await screen.findByDisplayValue("Kamppi");
    const grid = screen.getByRole("grid");
    grid.focus();
    // id, type, name, availableBikeNumber: the fourth column is not in the person's grant.
    for (let step = 0; step < 4; step++) fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.keyDown(grid, { key: "7" });
    expect(screen.getByText(`${DEFAULT_LABELS.locked}: ${DEFAULT_INSPECTOR_LABELS.notInGrant}`, { selector: "[role=status]" })).toBeInTheDocument();
    expect(screen.queryByDisplayValue("7")).toBeNull();
  });

  it("counts pasted cells the person may not change as not allowed", async () => {
    const { source } = recorded();
    render(<EntityGrid config={configOf()} source={source} access={ACCESS} />);
    await screen.findByDisplayValue("Kamppi");
    const grid = screen.getByRole("grid");
    grid.focus();
    // id, type, then name: the paste starts on the name and runs over the bikes (no write) and the status (prohibited).
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.paste(grid, { clipboardData: { getData: () => "Kamppi 2\t7\tbroken\n" } });
    await waitFor(() => expect(screen.getByText(`1 ${DEFAULT_LABELS.pasted}, 2 ${DEFAULT_LABELS.notAllowed}`)).toBeInTheDocument());
    expect(screen.getByDisplayValue("Kamppi 2")).toBeInTheDocument();
    expect(screen.getByText("working")).toBeInTheDocument();
  });
});
