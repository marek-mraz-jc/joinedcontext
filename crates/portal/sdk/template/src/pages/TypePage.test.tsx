import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { Row, Schema } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "../App";
import { filtersOf, shapeOf } from "../components/shape";

vi.mock("maplibre-gl", () => ({
  Map: class {
    on = vi.fn();
    remove = vi.fn();
  },
  setWorkerUrl: vi.fn(),
}));
vi.mock("@deck.gl/mapbox", () => ({ MapboxOverlay: class {} }));
vi.mock("@deck.gl/layers", () => ({ ScatterplotLayer: class {} }));
vi.mock("@deck.gl/aggregation-layers", () => ({ HexagonLayer: class {}, GridLayer: class {} }));
vi.mock("echarts", () => ({ init: vi.fn(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn(), on: vi.fn() })) }));

const STATIONS: Row[] = [
  { id: "urn:ngsi-ld:Station:example.org:demo:1", type: "Station", name: "Kamppi", bikes: 4, status: "open", location: { type: "Point", coordinates: [24.93, 60.17] } },
  { id: "urn:ngsi-ld:Station:example.org:demo:2", type: "Station", name: "Kallio", bikes: 7, status: "closed", location: { type: "Point", coordinates: [24.95, 60.18] } },
];
const NOTES: Row[] = [{ id: "urn:ngsi-ld:Note:example.org:demo:1", type: "Note", text: "check the lock" }];

// Shaped like Model Tools' draft-07 schema: the kind travels as `x-ngsi-ld-kind`.
const SCHEMA = {
  Station: {
    properties: {
      name: { type: "string" },
      bikes: { type: "integer", minimum: 0 },
      status: { type: "string", enum: ["open", "closed"] },
      location: { type: "object", "x-ngsi-ld-kind": "GeoProperty" },
    },
    required: ["name"],
  },
  Note: { properties: { text: { type: "string" } } },
} as Schema;

// Edit is offered to a signed-in reader whose grants allow the write (SDK-40).
const PERSON = { id: "u1", name: "Aino", roles: ["steward"] };

const ACCESS = {
  permissions: [
    { resource: { type: "*" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const },
    { resource: { type: "Station" }, actions: ["updateAttrs"], attributes: ["bikes", "status"] },
  ],
  prohibitions: [],
};

function app() {
  const client = stubClient({
    entities: [...STATIONS, ...NOTES],
    schema: SCHEMA,
    access: ACCESS,
    functions: { summary: () => ({ types: [{ type: "Note", count: 1, averages: {} }] }) },
  }, { appName: "bikes", user: PERSON });
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

describe("template", () => {
  it("opens on an overview with a card per type and the server summary as a chart", async () => {
    app();

    expect(await screen.findByRole("heading", { name: "bikes" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Overview" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "Station" })).toBeInTheDocument();
    expect(await screen.findByText("Entities per type")).toBeInTheDocument();
    expect(screen.queryByRole("list")).not.toBeInTheDocument();
    // The summary's numbers reach a screen reader through the chart's table (T-2974).
    const summary = within(screen.getByRole("figure", { name: "Entities per type" })).getByRole("table");
    expect(within(summary).getByRole("row", { name: "Note 1" })).toBeInTheDocument();
  });

  it("AP-138: the overview maps located data and charts every type that has something to chart", async () => {
    app();

    const station = (await screen.findByRole("heading", { name: "Station" })).closest("article") as HTMLElement;
    expect(station).not.toBeNull();
    await waitFor(() => expect(within(station).getByTestId("jc-map")).toBeInTheDocument());
    // The canvas is there at once; the legend and the chart wait for the rows they are drawn
    // from, so they are awaited too (T-2973: a loaded CI runner read the legend too early).
    expect(await within(station).findByRole("img", { name: "bikes: 4 to 7" })).toBeInTheDocument();
    await waitFor(() => expect(station.querySelector(".jc-chart-canvas")).not.toBeNull());
    // A type without a location gets no map.
    const note = screen.getByRole("heading", { name: "Note" }).closest("article") as HTMLElement;
    expect(within(note).queryByTestId("jc-map")).not.toBeInTheDocument();
  });

  it("a type with a location gets a map, filters from the schema, and a row opens the entity panel that edits where writes are granted", async () => {
    const client = app();
    fireEvent.click(await screen.findByRole("button", { name: "Station" }));

    const page = screen.getByRole("region", { name: "Station" });
    expect(await within(page).findByRole("button", { name: "Kallio" })).toBeInTheDocument();
    expect(within(page).getByTestId("jc-map")).toBeInTheDocument();
    expect(within(page).getAllByRole("combobox")[0]).toBeInTheDocument(); // the status select

    fireEvent.click(await within(page).findByRole("button", { name: "Kallio" }));
    const panel = await screen.findByRole("dialog", { name: "Kallio" });
    fireEvent.click(await within(panel).findByRole("button", { name: "Edit" }));
    // Only the granted attributes are inputs; `name` is shown, not edited.
    expect(within(panel).queryByLabelText("Name")).not.toBeInTheDocument();
    fireEvent.change(within(panel).getByLabelText("Bikes"), { target: { value: "9" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    expect(within(panel).getByText("Bikes: 7 → 9")).toBeInTheDocument();
    fireEvent.click(within(panel).getByRole("button", { name: "Save the change" }));

    await waitFor(() => expect(client.transport.rows().find((r) => r.id === STATIONS[1].id)?.bikes).toBe(9));
  });

  it("a type without a location or a write grant gets no map, and its panel no edit button", async () => {
    app();
    fireEvent.click(await screen.findByRole("button", { name: "Note" }));

    const page = screen.getByRole("region", { name: "Note" });
    expect(await within(page).findByText("check the lock")).toBeInTheDocument();
    fireEvent.click(within(within(page).getByRole("grid")).getAllByRole("button").find((each) => each.className === "jc-open") as HTMLElement);
    expect(within(page).queryByTestId("jc-map")).not.toBeInTheDocument();
    const panel = await screen.findByRole("dialog");
    expect(await within(panel).findByText("check the lock")).toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
  });
});

// SDK-02 (T-2724): an App reading several endpoints reads, checks grants and writes each type
// through the endpoint that serves it; a type two endpoints serve gets a page per endpoint.
describe("template over several endpoints", () => {
  // AppShell opens the page the address names; each test starts on the overview.
  beforeEach(() => window.history.replaceState(null, "", "/"));

  const GRANT: typeof ACCESS = {
    permissions: [{ resource: { type: "*" }, actions: ["queryEntity", "retrieveEntity", "updateAttrs"], attributes: "*" }],
    prohibitions: [],
  };
  const READ_ONLY: typeof ACCESS = {
    permissions: [{ resource: { type: "*" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }],
    prohibitions: [],
  };

  function severalApp(endpoints: { name: string; types: string[] }[], grants: Record<string, typeof ACCESS>) {
    const client = stubClient(
      {
        entities: [...STATIONS, ...NOTES],
        schema: SCHEMA,
        access: READ_ONLY,
        functions: { summary: () => ({ types: [] }) },
        refuse: (request) => {
          const slug = /^\/api\/endpoint\/([^/]+)\/access$/.exec(request.path)?.[1];
          return slug && grants[slug] ? { status: 200, body: grants[slug] } : null;
        },
      },
      { appName: "bikes", user: PERSON, endpoints: endpoints.map((e) => ({ ...e, slug: e.name, space: "demo" })) },
    );
    render(
      <JcProvider client={client}>
        <App />
      </JcProvider>,
    );
    return client;
  }

  it("reads a type and its grants through the endpoint serving it, not the primary", async () => {
    const client = severalApp([{ name: "a", types: ["Note"] }, { name: "b", types: ["Station"] }], { b: GRANT });
    expect(await screen.findAllByText("From b")).toHaveLength(1);
    fireEvent.click(await screen.findByRole("button", { name: "Station" }));

    const page = screen.getByRole("region", { name: "Station" });
    fireEvent.click(await within(page).findByRole("button", { name: "Kallio" }));
    // Endpoint a grants no write; b does, and b serves Station.
    const panel = await screen.findByRole("dialog", { name: "Kallio" });
    expect(await within(panel).findByRole("button", { name: "Edit" })).toBeInTheDocument();
    const paths = client.transport.calls.map((call) => call.path);
    expect(paths.some((path) => path.startsWith("/api/endpoint/b/ngsi-ld/v1/entities?") && path.includes("type=Station"))).toBe(true);
    expect(paths.some((path) => path.startsWith("/api/endpoint/a/ngsi-ld/v1/entities?") && path.includes("type=Station"))).toBe(false);
  });

  it("gives a type two endpoints serve a page per endpoint, each reading its own", async () => {
    const client = severalApp([{ name: "north", types: ["Station", "Note"] }, { name: "south", types: ["Station"] }], {});
    fireEvent.click(await screen.findByRole("button", { name: "Station (south)" }));

    expect(screen.getByRole("region", { name: "Station (south)" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Station (north)" })).toBeInTheDocument();
    await waitFor(() =>
      expect(client.transport.calls.some((call) => call.path.startsWith("/api/endpoint/south/ngsi-ld/v1/entities?"))).toBe(true),
    );
    expect(window.location.hash).toBe("#/Station@south");
    // No page shows the SDK's refusal of a type it cannot place.
    expect(screen.queryByText(/served by more than one endpoint/)).not.toBeInTheDocument();
  });
});

describe("shape", () => {
  it("reads kinds from the schema and falls back to the rows without one", () => {
    expect(shapeOf(SCHEMA.Station, STATIONS)).toMatchObject({ geo: "location", numbers: ["bikes"], categories: ["status"], texts: ["name"], label: "name" });
    expect(shapeOf(undefined, STATIONS)).toMatchObject({ geo: "location", numbers: ["bikes"], label: "name" });
    expect(shapeOf(undefined, [])).toMatchObject({ attrs: [], numbers: [] });
  });

  it("builds a search, a select, a range and a date range", () => {
    const shape = { attrs: [], numbers: ["bikes"], categories: ["status", "zone", "extra"], texts: ["name"], time: "observedAt" };
    expect(filtersOf(shape).map((f) => f.kind)).toEqual(["search", "select", "select", "range", "dateRange"]);
  });
});
