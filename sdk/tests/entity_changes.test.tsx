// T-6333: the row dialog's Changes tab reads who changed what and when from the kept versions,
// newest first, with filters, CSV and a mark beside each changed field; a caller without
// readVersions (403) sees no tab and no mark.
import { describe, expect, it, vi } from "vitest";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { EntityGrid } from "../src/grid/EntityGrid";
import { EntityChanges } from "../src/grid/EntityChanges";
import { parseGridConfig } from "../src/grid/config";
import { changesCsv, changesOf, filterChanges } from "../src/grid/changes";
import type { EntityChange } from "../src/grid/changes";
import { endpointSource, fixtureSource, SourceError } from "../src/grid/source";
import type { EntitySource } from "../src/grid/source";

const ID = "urn:ngsi-ld:Station:st-1";
const jana = {
  actor: "user:jana",
  actorKind: "person",
  organization: "bb",
  channel: "ui",
};

/** A rename kept as versions (ADR-N-054 2.9) and a measurement corrected once beside plain readings. */
const answer = {
  id: ID,
  type: "Station",
  name: [
    {
      type: "Property",
      value: "Hlavná stanica",
      instanceId: "i1",
      provenance: {
        actor: "serviceAccount:pl-stations",
        actorKind: "pipeline",
        organization: "hel",
        operation: "create",
        writtenAt: "2026-10-01T08:00:00Z",
        audit: true,
      },
    },
    {
      type: "Property",
      value: "Stanica Centrum",
      instanceId: "i2",
      provenance: {
        ...jana,
        reason: "Renamed by the council",
        writtenAt: "2026-10-05T09:00:00Z",
        audit: true,
      },
    },
    {
      type: "Property",
      value: "Stanica Námestie",
      instanceId: "i3",
      provenance: {
        ...jana,
        actor: "user:peter",
        reason: "Typo",
        writtenAt: "2026-10-07T09:00:00Z",
        audit: true,
      },
    },
  ],
  temperature: [
    {
      type: "Property",
      value: 12,
      observedAt: "2026-10-06T10:00:00Z",
      provenance: { channel: "pipeline", writtenAt: "2026-10-06T10:00:05Z" },
    },
    {
      type: "Property",
      value: 13,
      observedAt: "2026-10-06T10:00:00Z",
      provenance: {
        ...jana,
        reason: "Sensor fixed",
        writtenAt: "2026-10-06T11:00:00Z",
      },
    },
    {
      type: "Property",
      value: 14,
      observedAt: "2026-10-06T11:00:00Z",
      provenance: { channel: "pipeline", writtenAt: "2026-10-06T11:00:05Z" },
    },
  ],
};

describe("changesOf (T-6333)", () => {
  it("lists each replaced value newest first, corrections of a reading only, and drops what has no time", () => {
    const changes = changesOf({ ...answer, broken: [{ value: 1 }] });
    expect(changes.map((c) => [c.attr, c.at])).toEqual([
      ["name", "2026-10-07T09:00:00Z"],
      ["temperature", "2026-10-06T11:00:00Z"],
      ["name", "2026-10-05T09:00:00Z"],
    ]);
    expect(changes[0].before.value).toBe("Stanica Centrum");
    expect(changes[0].after.value).toBe("Stanica Námestie");
    expect(changes[1].before.value).toBe(12);
    expect(changes[2].provenance.reason).toBe("Renamed by the council");
    expect(changesOf(null)).toEqual([]);
  });

  it("filters by attribute and by actor, and writes the shown rows as CSV safe for a spreadsheet", () => {
    const changes = changesOf(answer);
    expect(filterChanges(changes, "name", null)).toHaveLength(2);
    expect(
      filterChanges(changes, null, "user:peter").map((c) => c.after.value),
    ).toEqual(["Stanica Námestie"]);
    const evil: EntityChange = {
      ...changes[0],
      after: { value: '=HYPERLINK("x")' },
    };
    const csv = changesCsv([evil]).split("\n");
    expect(csv[0]).toBe(
      "writtenAt,attribute,datasetId,before,after,actor,actorKind,organization,channel,reason",
    );
    expect(csv[1]).toContain(`"'=HYPERLINK(""x"")"`);
    expect(csv[1]).toContain("user:peter,person,bb,ui,Typo");
  });
});

describe("the source's changes read (T-6333)", () => {
  it("asks for every version of the row's attributes, and reads a 404 as nothing kept", async () => {
    const paths: string[] = [];
    const transport = vi.fn(async (request: { path: string }) => {
      paths.push(request.path);
      return paths.length === 1
        ? { status: 200, body: answer }
        : { status: 404, body: {} };
    });
    const source = endpointSource("stations", transport);
    expect(await source.changes?.(ID, ["name", "temperature"])).toHaveLength(3);
    expect(await source.changes?.(ID, ["name"])).toEqual([]);
    const url = new URL(paths[0], "https://x");
    expect(url.pathname).toBe(
      `/api/endpoint/stations/ngsi-ld/v1/temporal/entities/${encodeURIComponent(ID)}`,
    );
    expect(url.searchParams.get("versions")).toBe("all");
    expect(url.searchParams.get("attrs")).toBe("name,temperature");
  });

  it("does not ask for versions on a plain grid read", async () => {
    const paths: string[] = [];
    const source = endpointSource(
      "stations",
      async (request: { path: string }) => {
        paths.push(request.path);
        return { status: 200, body: [] };
      },
    );
    await source.query({ type: "Station" }, { offset: 0, limit: 10 });
    expect(paths.every((path) => !path.includes("versions"))).toBe(true);
  });
});

const rows = [
  {
    id: ID,
    type: "Station",
    name: { type: "Property", value: "Stanica Námestie" },
    temperature: { type: "Property", value: 14 },
  },
];
const config = parseGridConfig({
  source: { kind: "fixture", name: "test" },
  type: "Station",
  columns: [
    { attr: "name", label: "Name", pinned: true },
    { attr: "temperature", label: "Temperature" },
  ],
  pageSize: 10,
  history: { enabled: true },
}).config!;

function sourceWith(changes: EntitySource["changes"]): EntitySource {
  return { ...fixtureSource(rows), history: async () => [], changes };
}

async function openRow() {
  fireEvent.click(
    await screen.findByRole("button", { name: "Open: Stanica Námestie" }),
  );
  return screen.getByRole("dialog", { name: "Details: Stanica Námestie" });
}

describe("the Changes tab (T-6333)", () => {
  it("shows the timeline with the grid's cell renderer, filters it, and a field's mark opens it on that field", async () => {
    const changes = vi.fn(async () => changesOf(answer));
    render(<EntityGrid config={config} source={sourceWith(changes)} />);
    const dialog = await openRow();
    await waitFor(() =>
      expect(
        within(dialog).getByRole("tab", { name: "Changes" }),
      ).toBeInTheDocument(),
    );
    expect(changes).toHaveBeenCalledTimes(1);
    expect(changes).toHaveBeenCalledWith(ID, ["name", "temperature"]);

    fireEvent.click(
      within(dialog).getByRole("button", { name: "Show the changes of Name" }),
    );
    const panel = within(dialog).getByRole("tabpanel");
    expect(
      within(dialog).getByRole("tab", { name: "Changes" }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      within(panel).getByRole("combobox", { name: "Attribute" }),
    ).toHaveValue("name");
    let items = within(panel).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent("Stanica Centrum");
    expect(items[0]).toHaveTextContent("Stanica Námestie");
    expect(items[0]).toHaveTextContent(
      "Changed by: peter (person)Organization: bbThrough: ui",
    );
    expect(items[0]).toHaveTextContent("Reason: Typo");

    fireEvent.change(
      within(panel).getByRole("combobox", { name: "Attribute" }),
      { target: { value: "" } },
    );
    items = within(panel).getAllByRole("listitem");
    expect(
      items.map((item) => item.querySelector("strong")?.textContent),
    ).toEqual(["Name", "Temperature", "Name"]);
    fireEvent.change(
      within(panel).getByRole("combobox", { name: "Changed by" }),
      { target: { value: "user:jana" } },
    );
    expect(within(panel).getAllByRole("listitem")).toHaveLength(2);

    const writeText = vi.fn(async () => undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    fireEvent.click(within(panel).getByRole("button", { name: "Copy as CSV" }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    const lines = String((writeText.mock.calls[0] as unknown[])[0]).split("\n");
    expect(lines).toHaveLength(3);
  });

  it("hides the tab and the marks from a caller without readVersions", async () => {
    const changes = vi.fn(async () => {
      throw new SourceError(403, "the space's policy grants no readVersions");
    });
    render(<EntityGrid config={config} source={sourceWith(changes)} />);
    const dialog = await openRow();
    await waitFor(() => expect(changes).toHaveBeenCalled());
    await waitFor(() =>
      expect(
        within(dialog)
          .getAllByRole("tab")
          .map((tab) => tab.textContent),
      ).toEqual(["Fields", "History", "Metadata"]),
    );
    expect(
      within(dialog).queryByRole("button", { name: /Show the changes/ }),
    ).toBeNull();
    expect(dialog).not.toHaveTextContent("readVersions");
  });

  it("says nothing is kept yet, and offers Try again after a failure", async () => {
    let fail = true;
    const changes = vi.fn(async () => {
      if (fail) throw new SourceError(502, "upstream");
      return [];
    });
    render(<EntityGrid config={config} source={sourceWith(changes)} />);
    const dialog = await openRow();
    fireEvent.click(
      await within(dialog).findByRole("tab", { name: "Changes" }),
    );
    const retry = await within(dialog).findByRole("button", {
      name: "Try again",
    });
    expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "The changes could not be read.",
    );
    fail = false;
    fireEvent.click(retry);
    expect(
      await within(dialog).findByText(/No change of this row is kept yet/),
    ).toBeInTheDocument();
  });
});

describe("EntityChanges alone (T-6333)", () => {
  it("tells two writers with one short name apart, and its reset clears both filters", () => {
    const changes = changesOf(answer).map((change, index) =>
      index === 1
        ? {
            ...change,
            provenance: { ...change.provenance, actor: "serviceAccount:jana" },
          }
        : change,
    );
    const attrs = [
      { attr: "name", label: "Name" },
      { attr: "temperature", label: "Temperature" },
    ];
    let attr: string | null = "temperature";
    const { rerender } = render(
      <EntityChanges
        state={{ status: "ready", changes }}
        attr={attr}
        onAttr={(next) => (attr = next)}
        attrs={attrs}
        renderValue={(_name, instance) => String(instance.value)}
        locale="en"
      />,
    );
    const person = screen.getByRole("combobox", { name: "Changed by" });
    expect(
      within(person)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["All", "peter", "serviceAccount:jana", "user:jana"]);
    fireEvent.change(person, { target: { value: "user:jana" } });
    // The correction of temperature is serviceAccount:jana's: nothing of user:jana's is left.
    expect(
      screen.getByText("No change matches these filters."),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show every change" }));
    expect(attr).toBeNull();
    rerender(
      <EntityChanges
        state={{ status: "ready", changes }}
        attr={attr}
        onAttr={(next) => (attr = next)}
        attrs={attrs}
        renderValue={(_name, instance) => String(instance.value)}
        locale="en"
      />,
    );
    expect(screen.getAllByRole("listitem")).toHaveLength(3);
  });
});
