import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { GridColumn, Row, Schema } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { TypeGrid } from "./TypeGrid";

const STATIONS: Row[] = [
  { id: "urn:ngsi-ld:Station:example.org:demo:1", type: "Station", name: "Kamppi", bikes: 4, slots: 8 },
  { id: "urn:ngsi-ld:Station:example.org:demo:2", type: "Station", name: "Kallio", bikes: 0, slots: 5 },
];

const SCHEMA = {
  Station: {
    properties: {
      name: { type: "string" },
      bikes: { type: "integer" },
      slots: { type: "integer" },
      location: { type: "object", "x-ngsi-ld-kind": "GeoProperty" },
      owner: { type: "string", "x-ngsi-ld-kind": "Relationship" },
    },
  },
} as Schema;

function shown(columns?: GridColumn[], onSelect?: (id: string) => void, schema?: Schema) {
  const client = stubClient({ entities: STATIONS, ...(schema ? { schema } : {}) });
  render(
    <JcProvider client={client}>
      <TypeGrid type="Station" columns={columns} onSelect={onSelect} />
    </JcProvider>,
  );
  return client;
}

describe("TypeGrid", () => {
  it("lists the type's rows in the SDK grid, read at the endpoint", async () => {
    const client = shown();
    const grid = await screen.findByRole("grid");
    expect(await within(grid).findByText("Kamppi")).toBeInTheDocument();
    expect(client.transport.calls.some((call) => call.path.includes("options=sysAttrs"))).toBe(true);
  });

  it("reads only the rows its q keeps, at the endpoint", async () => {
    const client = stubClient({ entities: STATIONS });
    render(
      <JcProvider client={client}>
        <TypeGrid type="Station" q="bikes==0" />
      </JcProvider>,
    );
    await screen.findByRole("grid");
    const reads = client.transport.calls.filter((call) => call.path.includes("/entities"));
    expect(reads.length).toBeGreaterThan(0);
    expect(reads.every((call) => decodeURIComponent(call.path).includes("q=bikes==0"))).toBe(true);
  });

  it("works a formula column out of each row", async () => {
    shown([{ attr: "name" }, { attr: "occupancy", formula: "bikes / slots * 100" }]);
    expect(await screen.findByText("50")).toBeInTheDocument();
  });

  it("opens a row by its name", async () => {
    const onSelect = vi.fn();
    shown(undefined, onSelect);
    fireEvent.click(await screen.findByRole("button", { name: "Kallio" }));
    expect(onSelect).toHaveBeenCalledWith(STATIONS[1].id);
  });

  it("shows a value beside the name: no type, no geometry, no references, no query box", async () => {
    shown(undefined, undefined, SCHEMA);
    const grid = await screen.findByRole("grid");
    expect(await within(grid).findByText("Kamppi")).toBeInTheDocument();
    const heads = within(grid).getAllByRole("columnheader").map((head) => head.textContent ?? "");
    expect(heads.some((head) => head.includes("Bikes"))).toBe(true);
    expect(heads.some((head) => /Type|Location|Owner/.test(head))).toBe(false);
    expect(heads.filter((head) => head.includes("Name"))).toHaveLength(1);
    expect(screen.queryByText("The query this asks")).toBeNull();
  });

  it("names each cell by its column for the phone's card, with no swipe cue", async () => {
    shown(undefined, undefined, SCHEMA);
    const grid = await screen.findByRole("grid");
    const cell = (await within(grid).findByText("4")).closest("td");
    expect(cell).toHaveAttribute("data-label", "Bikes");
    expect(grid.closest(".jc-type-grid")).not.toBeNull();
    expect(screen.queryByText(/swipe/i)).toBeNull();
  });

  it("filters by the name it leads with, not by the id", async () => {
    shown(undefined, undefined, SCHEMA);
    const grid = await screen.findByRole("grid");
    await within(grid).findByText("Kamppi");
    const heads = within(grid).getAllByRole("columnheader");
    expect(heads[0]).toHaveAttribute("data-col-attr", "name");
    expect(within(heads[0]).getByRole("button", { name: /filter/i })).toBeInTheDocument();
  });

  it("still reads the name when the listed columns leave it out", async () => {
    shown([{ attr: "bikes" }], undefined, SCHEMA);
    const grid = await screen.findByRole("grid");
    expect(await within(grid).findByText("Kamppi")).toBeInTheDocument();
    expect(within(grid).getByText("4")).toBeInTheDocument();
  });

  it("says what is wrong with a column it cannot draw", async () => {
    shown([{ attr: "occupancy", formula: "fetch('x')" }]);
    expect(await screen.findByRole("alert")).toHaveTextContent("formula");
  });
});
