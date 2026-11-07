/**
 * Edit mode in the grid (T-1430; UI-67, EP-55, AG-78, UI-60): a person corrects one value by hand,
 * reads what they are about to send, and the Endpoint decides it.
 *
 * Every case is about the two things a person can lose: the value they typed, and the truth about
 * whether it landed. A refused update therefore keeps its cell and its reason, an applied one is
 * re-read from the endpoint, and nothing is sent for a surface that takes no writes.
 */
import { describe, expect, it } from "vitest";
import { act, render, screen, waitFor, fireEvent, within } from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { parseGridConfig } from "../src/grid/config";
import { fixtureSource, SourceError } from "../src/grid/source";
import { DEFAULT_LABELS } from "../src/grid/useEntityGrid";

const entities: Record<string, unknown>[] = [
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
    type: "BikeHireDockingStation",
    availableBikeNumber: { type: "Property", value: 5, unitCode: "C62", observedAt: "2026-01-01T10:00:00Z" },
    name: { type: "Property", value: "Kamppi" },
  },
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:002",
    type: "BikeHireDockingStation",
    availableBikeNumber: { type: "Property", value: 3, unitCode: "C62" },
    name: { type: "Property", value: "Kallio" },
  },
];

function configOf(extra: Record<string, unknown> = {}) {
  const parsed = parseGridConfig({
    source: { kind: "fixture", name: "test" },
    type: "BikeHireDockingStation",
    columns: [
      { attr: "name", label: "Name" },
      { attr: "availableBikeNumber", label: "Bikes", show: { unit: true } },
    ],
    mode: "edit",
    // `parseGridConfig` requires the list in edit mode: a grid opens the columns someone named.
    editableAttrs: ["availableBikeNumber", "name"],
    pageSize: 10,
    ...extra,
  });
  expect(parsed.findings, JSON.stringify(parsed.findings)).toEqual([]);
  return parsed.config!;
}

/** The fixture source plus the one capability edit mode needs, recording what it is asked to write. */
function writable(refuse?: (id: string) => SourceError | undefined) {
  const inner = fixtureSource(entities);
  const written: { id: string; attrs: Record<string, unknown> }[] = [];
  const reasons: (string | undefined)[] = [];
  const reads = { count: 0 };
  return {
    written,
    reasons,
    reads,
    source: {
      query: async (...args: Parameters<typeof inner.query>) => {
        reads.count += 1;
        return inner.query(...args);
      },
      get: inner.get,
      patch: async (id: string, attrs: Record<string, unknown>, reason?: string) => {
        written.push({ id, attrs });
        reasons.push(reason);
        const problem = refuse?.(id);
        if (problem) {
          throw problem;
        }
      },
    },
  };
}

describe("edit mode", () => {
  it("edits a cell, reviews it, and sends one update the endpoint decides", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("Kamppi")).toBeInTheDocument());

    // The person corrects one value; nothing is sent while they type.
    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    expect(written).toHaveLength(0);
    expect(screen.getByText(`1 ${DEFAULT_LABELS.pending}`)).toBeInTheDocument();

    // What they are about to send, in the words of the grid they were reading.
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    // Each column says what it holds: the entity, the attribute, the stored value, the new one.
    expect(within(panel).getAllByRole("columnheader").map((header) => header.textContent)).toEqual([
      DEFAULT_LABELS.entity,
      DEFAULT_LABELS.attribute,
      DEFAULT_LABELS.before,
      DEFAULT_LABELS.after,
    ]);
    // The row's name and the column's label, never the URN (T-5530): that sits behind its copy button.
    expect(within(panel).getByRole("row", { name: /Kamppi/ }).textContent).toContain("Bikes");
    expect(panel.textContent).not.toContain("urn:ngsi-ld:");
    expect(within(panel).getByRole("button", { name: `${DEFAULT_LABELS.copyId}: Kamppi` })).toBeInTheDocument();

    fireEvent.click(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply }));
    await waitFor(() => expect(written).toHaveLength(1));
    // One entity, one PATCH, the number as a number and the unit kept.
    expect(written[0]).toEqual({
      id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
      attrs: { availableBikeNumber: { type: "Property", value: 4, unitCode: "C62" } },
    });
    // Applied cells are no longer pending, and the endpoint's own answer is read again.
    await waitFor(() => expect(screen.queryByText(`1 ${DEFAULT_LABELS.pending}`)).toBeNull());
  });

  it("marks a value its model refuses at the cell and holds Apply until it is corrected", async () => {
    const { written, source } = writable();
    render(
      <EntityGrid config={configOf()} source={source} rules={{ availableBikeNumber: { kind: "integer", minimum: 0 } }} />,
    );
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "-1" } });
    const cell = screen.getByDisplayValue("-1");
    // The reason sits beside the input and is what a screen reader hears with it.
    expect(cell).toHaveAttribute("aria-invalid", "true");
    expect(cell).toHaveAccessibleDescription(`${DEFAULT_LABELS.atLeast} 0`);
    expect(screen.getByText(`1 ${DEFAULT_LABELS.toCorrect}`)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: DEFAULT_LABELS.review })).toBeDisabled();

    fireEvent.change(cell, { target: { value: "2" } });
    expect(screen.getByDisplayValue("2")).not.toHaveAttribute("aria-invalid");
    expect(screen.queryByText(`1 ${DEFAULT_LABELS.toCorrect}`)).toBeNull();
    expect(screen.getByRole("button", { name: DEFAULT_LABELS.review })).toBeEnabled();
    expect(written).toHaveLength(0);
  });

  it("edits each attribute in the input its model's range asks for and sends the value that range means", async () => {
    const typed = [
      {
        id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
        type: "BikeHireDockingStation",
        name: { type: "Property", value: "Kamppi" },
        open: { type: "Property", value: true },
        since: { type: "Property", value: "2020-05-01" },
        site: { type: "Property", value: "https://hel.fi/kamppi" },
      },
    ];
    const inner = fixtureSource(typed);
    const written: { id: string; attrs: Record<string, unknown> }[] = [];
    const source = { ...inner, patch: async (id: string, attrs: Record<string, unknown>) => void written.push({ id, attrs }) };
    const config = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "BikeHireDockingStation",
      columns: [
        { attr: "name", label: "Name" },
        { attr: "open", label: "Open" },
        { attr: "since", label: "Since" },
        { attr: "site", label: "Site" },
        { attr: "capacity", label: "Capacity" },
      ],
      mode: "edit",
      editableAttrs: ["name", "open", "since", "site", "capacity"],
    }).config!;
    render(
      <EntityGrid
        config={config}
        source={source}
        rules={{
          name: { kind: "string" },
          open: { kind: "boolean" },
          since: { kind: "date" },
          site: { kind: "uri" },
          capacity: { kind: "integer" },
        }}
      />,
    );
    await waitFor(() => expect(screen.getByDisplayValue("Kamppi")).toBeInTheDocument());

    expect(screen.getByLabelText(`${DEFAULT_LABELS.edit} Since`)).toHaveAttribute("type", "date");
    expect(screen.getByLabelText(`${DEFAULT_LABELS.edit} Site`)).toHaveAttribute("type", "url");
    expect(screen.getByLabelText(`${DEFAULT_LABELS.edit} Capacity`)).toHaveAttribute("type", "number");
    // A true/false attribute is picked, never typed.
    const open = screen.getByLabelText(`${DEFAULT_LABELS.edit} Open`);
    expect(open.tagName).toBe("SELECT");
    expect(within(open).getAllByRole("option").map((o) => o.textContent)).toEqual([DEFAULT_LABELS.yes, DEFAULT_LABELS.no]);

    fireEvent.change(open, { target: { value: "false" } });
    // "true" typed into a text attribute is the word, not a boolean.
    fireEvent.change(screen.getByDisplayValue("Kamppi"), { target: { value: "true" } });
    // An attribute the entity does not hold yet still gets a number, because its range says so.
    fireEvent.change(screen.getByLabelText(`${DEFAULT_LABELS.edit} Capacity`), { target: { value: "12" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    fireEvent.click(within(screen.getByRole("region", { name: DEFAULT_LABELS.review })).getByRole("button", { name: DEFAULT_LABELS.apply }));

    await waitFor(() => expect(written).toHaveLength(1));
    const attrs = written[0].attrs as Record<string, { value: unknown }>;
    expect(attrs.open.value).toBe(false);
    expect(attrs.name.value).toBe("true");
    expect(attrs.capacity.value).toBe(12);
  });

  it("dates the corrected values to now when the person says they were observed now", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    fireEvent.click(within(panel).getByLabelText(DEFAULT_LABELS.observedNow));
    fireEvent.click(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply }));

    await waitFor(() => expect(written).toHaveLength(1));
    const sent = written[0].attrs.availableBikeNumber as { observedAt?: string };
    expect(typeof sent.observedAt).toBe("string");
    expect(new Date(sent.observedAt!).getTime()).not.toBeNaN();
  });

  it("keeps a refused cell and its reason, and forgets the one that landed", async () => {
    const second = "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:002";
    const { written, source } = writable((id) =>
      id === second ? new SourceError(403, "your role may not write availableBikeNumber") : undefined,
    );
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    fireEvent.change(screen.getByDisplayValue("3"), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    fireEvent.click(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply }));

    await waitFor(() => expect(written).toHaveLength(2));
    // The endpoint's own sentence, where the person can see it, and the value still theirs.
    // In the review and at the cell itself (T-4826).
    expect(await screen.findAllByText(new RegExp("may not write availableBikeNumber"))).toHaveLength(2);
    await waitFor(() => expect(screen.getByText(`1 ${DEFAULT_LABELS.pending}`)).toBeInTheDocument());
    expect(screen.getByDisplayValue("2")).toBeInTheDocument();
  });

  // T-3097: every applied value leaves the pending list. Clearing them one call at a time kept all
  // but the last pending, so a grid that applied two entities still offered one of them again.
  it("forgets every cell that landed when several entities are applied at once", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    fireEvent.change(screen.getByDisplayValue("3"), { target: { value: "2" } });
    await waitFor(() => expect(screen.getByText(`2 ${DEFAULT_LABELS.pending}`)).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    fireEvent.click(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply }));

    await waitFor(() => expect(written).toHaveLength(2));
    await waitFor(() => expect(screen.queryByText(new RegExp(DEFAULT_LABELS.pending))).toBeNull());
  });

  it("forgets a cell typed back to what the endpoint answered", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    expect(screen.getByText(`1 ${DEFAULT_LABELS.pending}`)).toBeInTheDocument();
    fireEvent.change(screen.getByDisplayValue("4"), { target: { value: "5" } });
    // Nothing pending, so Apply is not even offered: there is nothing to apply.
    expect(screen.queryByText(`1 ${DEFAULT_LABELS.pending}`)).toBeNull();
    expect(screen.queryByRole("button", { name: DEFAULT_LABELS.review })).toBeNull();
    expect(written).toHaveLength(0);
  });

  it("discards every pending cell when the person asks", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.discard }));
    expect(screen.queryByText(`1 ${DEFAULT_LABELS.pending}`)).toBeNull();
    expect(screen.getByDisplayValue("5")).toBeInTheDocument();
    expect(written).toHaveLength(0);
  });

  it("offers no editing on a surface that takes no writes, or outside edit mode", async () => {
    // A viewer reads through a source without `patch`: there is nothing to type into.
    render(<EntityGrid config={configOf()} source={fixtureSource(entities)} />);
    await waitFor(() => expect(screen.getByText("Kamppi")).toBeInTheDocument());
    expect(screen.queryByLabelText(`${DEFAULT_LABELS.edit} Bikes`)).toBeNull();

    const { source } = writable();
    render(<EntityGrid config={configOf({ mode: "view", editableAttrs: [] })} source={source} />);
    await waitFor(() => expect(screen.getAllByText("Kallio").length).toBeGreaterThan(0));
    expect(screen.queryByLabelText(`${DEFAULT_LABELS.edit} Bikes`)).toBeNull();
  });

  it("edits only the attributes the config names", async () => {
    const { source } = writable();
    render(<EntityGrid config={configOf({ editableAttrs: ["availableBikeNumber"] })} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());
    // The name is read: a grid whose config opens one column does not open the rest.
    expect(screen.queryByLabelText(`${DEFAULT_LABELS.edit} Name`)).toBeNull();
    expect(screen.getByText("Kamppi")).toBeInTheDocument();
  });

  it("sends a value that looks like markup as data, and renders it as text", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf({ editableAttrs: ["name"] })} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("Kamppi")).toBeInTheDocument());

    fireEvent.change(screen.getByDisplayValue("Kamppi"), { target: { value: "<b>Kamppi</b>" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    fireEvent.click(
      within(screen.getByRole("region", { name: DEFAULT_LABELS.review })).getByRole("button", {
        name: DEFAULT_LABELS.apply,
      }),
    );
    await waitFor(() => expect(written).toHaveLength(1));
    expect(written[0].attrs).toEqual({ name: { type: "Property", value: "<b>Kamppi</b>" } });
    // As text: the markup is a value, never markup of the page.
    expect(document.querySelector("tbody b")).toBeNull();
  });

  it("refuses to apply more entities than the platform's own cap", async () => {
    const many = Array.from({ length: 51 }, (_, i) => ({
      id: `urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:${String(i).padStart(3, "0")}`,
      type: "BikeHireDockingStation",
      availableBikeNumber: { type: "Property", value: i },
    }));
    const inner = fixtureSource(many);
    const written: string[] = [];
    const source = {
      query: inner.query,
      get: inner.get,
      patch: async (id: string) => {
        written.push(id);
      },
    };
    render(
      <EntityGrid
        config={{ ...configOf({ editableAttrs: ["availableBikeNumber"] }), pageSize: 51 }}
        source={source}
      />,
    );
    await waitFor(() => expect(screen.getAllByLabelText(`${DEFAULT_LABELS.edit} Bikes`)).toHaveLength(51));

    for (const input of screen.getAllByLabelText(`${DEFAULT_LABELS.edit} Bikes`)) {
      fireEvent.change(input, { target: { value: "999" } });
    }
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    // Refused before anything is sent: half a batch is worse than none.
    expect(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply })).toBeDisabled();
    expect(written).toHaveLength(0);
  });

  it("re-reads the endpoint after applying, so the grid shows its answer and not the guess", async () => {
    const { source, reads } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    await waitFor(() => expect(screen.getByDisplayValue("5")).toBeInTheDocument());
    const before = reads.count;

    fireEvent.change(screen.getByDisplayValue("5"), { target: { value: "4" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    fireEvent.click(
      within(screen.getByRole("region", { name: DEFAULT_LABELS.review })).getByRole("button", {
        name: DEFAULT_LABELS.apply,
      }),
    );
    await waitFor(() => expect(reads.count).toBeGreaterThan(before));
  });
});

describe("a geometry cell in edit mode (T-1443, UI-72)", () => {
  const withGeometry = [
    {
      id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
      type: "BikeHireDockingStation",
      name: { type: "Property", value: "Kamppi" },
      location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.931, 60.169] } },
    },
  ];

  function geoConfig() {
    const parsed = parseGridConfig({
      source: { kind: "fixture", name: "test" },
      type: "BikeHireDockingStation",
      columns: [{ attr: "name", label: "Name" }, { attr: "location", label: "Location" }],
      mode: "edit",
      editableAttrs: ["name", "location"],
      pageSize: 10,
    });
    expect(parsed.findings, JSON.stringify(parsed.findings)).toEqual([]);
    return parsed.config!;
  }

  function source() {
    const inner = fixtureSource(withGeometry);
    return { ...inner, patch: async () => {} };
  }

  it("offers no text box over a geometry, because a text box would show and write [object Object]", async () => {
    render(<EntityGrid config={geoConfig()} source={source()} />);
    await waitFor(() => expect(screen.getByDisplayValue("Kamppi")).toBeInTheDocument());
    // The scalar column opens; the geometry column does not.
    expect(screen.queryByRole("textbox", { name: /Location/ })).toBeNull();
    expect(screen.queryByDisplayValue("[object Object]")).toBeNull();
    // The cell still says what it holds, so the table stays the record of the row.
    expect(screen.getByText("Point")).toBeInTheDocument();
  });
});

/** Reviews what is pending and applies it, with the reason the person typed when they give one. */
async function applyPending(reason?: string) {
  fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
  const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
  if (reason !== undefined) {
    fireEvent.change(within(panel).getByRole("textbox", { name: DEFAULT_LABELS.reason }), { target: { value: reason } });
  }
  fireEvent.click(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply }));
}

describe("editing from the keyboard and with a reason (T-4826, DM-113)", () => {
  it("makes the clicked cell active, and the keys go on from there", async () => {
    const { source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    const kallio = await screen.findByDisplayValue("Kallio");
    const grid = screen.getByRole("grid");
    fireEvent.mouseDown(kallio.closest('[role="gridcell"]')!);
    expect(grid.getAttribute("aria-activedescendant")).toBe(kallio.closest('[role="gridcell"]')!.id);
    grid.focus();
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    // The bikes cell of the same, second row: not row 0, where the old grid jumped (A04).
    const active = document.getElementById(grid.getAttribute("aria-activedescendant")!)!;
    expect(within(active).getByDisplayValue("3")).toBeInTheDocument();
  });

  it("starts editing with the character typed, and Escape puts the value back", async () => {
    const { source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    const five = await screen.findByDisplayValue("5");
    fireEvent.mouseDown(five.closest('[role="gridcell"]')!);
    const grid = screen.getByRole("grid");
    grid.focus();
    fireEvent.keyDown(grid, { key: "7" });
    const input = screen.getByDisplayValue("7");
    expect(input).toHaveFocus();
    expect(screen.getByText(`1 ${DEFAULT_LABELS.pending}`)).toBeInTheDocument();

    fireEvent.keyDown(input, { key: "Escape" });
    expect(grid).toHaveFocus();
    expect(screen.getByDisplayValue("5")).toBeInTheDocument();
    expect(screen.queryByText(`1 ${DEFAULT_LABELS.pending}`)).toBeNull();
  });

  it("keeps the value on Enter and moves down; a cell focused by Tab becomes the active one", async () => {
    const { source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    const grid = screen.getByRole("grid");
    const kamppi = await screen.findByDisplayValue("Kamppi");
    const activeText = () => document.getElementById(grid.getAttribute("aria-activedescendant")!)!.querySelector("input")?.value;

    act(() => kamppi.focus());
    expect(activeText()).toBe("Kamppi");
    fireEvent.change(kamppi, { target: { value: "Kamppi 2" } });
    fireEvent.keyDown(kamppi, { key: "Enter" });
    expect(grid).toHaveFocus();
    expect(activeText()).toBe("Kallio");
    expect(screen.getByDisplayValue("Kamppi 2")).toBeInTheDocument();

    // Tab is the browser's: the next cell's box takes the focus, and the active cell follows it.
    act(() => screen.getByDisplayValue("3").focus());
    expect(activeText()).toBe("3");
  });

  it("sends the default reason with each update, or the one the person typed", async () => {
    const first = writable();
    const { unmount } = render(<EntityGrid config={configOf()} source={first.source} />);
    fireEvent.change(await screen.findByDisplayValue("5"), { target: { value: "4" } });
    await applyPending();
    await waitFor(() => expect(first.reasons).toEqual([DEFAULT_LABELS.reasonDefault]));
    unmount();

    const second = writable();
    render(<EntityGrid config={configOf()} source={second.source} />);
    fireEvent.change(await screen.findByDisplayValue("5"), { target: { value: "4" } });
    await applyPending("  Counter reset at 06:00  ");
    await waitFor(() => expect(second.reasons).toEqual(["Counter reset at 06:00"]));
  });

  it("holds Apply while the reason is empty, and says why", async () => {
    const { written, source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    fireEvent.change(await screen.findByDisplayValue("5"), { target: { value: "4" } });
    fireEvent.click(screen.getByRole("button", { name: DEFAULT_LABELS.review }));
    const panel = screen.getByRole("region", { name: DEFAULT_LABELS.review });
    const field = within(panel).getByRole("textbox", { name: DEFAULT_LABELS.reason });
    fireEvent.change(field, { target: { value: "   " } });
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(within(panel).getByText(DEFAULT_LABELS.reasonNeeded)).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: DEFAULT_LABELS.apply })).toBeDisabled();
    expect(written).toHaveLength(0);
  });

  it("offers Undo after an apply, which writes the replaced value back with its own reason", async () => {
    const { written, reasons, source } = writable();
    render(<EntityGrid config={configOf()} source={source} />);
    fireEvent.change(await screen.findByDisplayValue("5"), { target: { value: "4" } });
    await applyPending("Sensor stuck");
    fireEvent.click(await screen.findByRole("button", { name: DEFAULT_LABELS.undo }));
    await waitFor(() => expect(written).toHaveLength(2));
    expect(written[1]).toEqual({
      id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
      attrs: { availableBikeNumber: { type: "Property", value: 5, unitCode: "C62" } },
    });
    expect(reasons[1]).toBe(`${DEFAULT_LABELS.undoReason}: Sensor stuck`);
    expect(await screen.findByText(DEFAULT_LABELS.undone)).toBeInTheDocument();
  });

  it("shows the gateway's 400 at the cell it names, and keeps the value typed", async () => {
    const { source } = writable(() => new SourceError(400, "availableBikeNumber must be at least 0", "availableBikeNumber"));
    render(<EntityGrid config={configOf()} source={source} />);
    fireEvent.change(await screen.findByDisplayValue("5"), { target: { value: "-1" } });
    await applyPending();
    const input = await screen.findByDisplayValue("-1");
    await waitFor(() => expect(input).toHaveAttribute("aria-invalid", "true"));
    const message = document.getElementById(input.getAttribute("aria-describedby")!);
    expect(message?.textContent).toBe("availableBikeNumber must be at least 0");
  });

  it("shows a 403 that names no attribute at each cell of the entity it refused", async () => {
    const { source } = writable(() => new SourceError(403, "your role may not write this entity"));
    render(<EntityGrid config={configOf()} source={source} />);
    fireEvent.change(await screen.findByDisplayValue("5"), { target: { value: "4" } });
    await applyPending();
    const input = await screen.findByDisplayValue("4");
    await waitFor(() => expect(input).toHaveAttribute("aria-invalid", "true"));
    expect(document.getElementById(input.getAttribute("aria-describedby")!)?.textContent).toBe("your role may not write this entity");
  });
});
