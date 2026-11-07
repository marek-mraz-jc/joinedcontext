import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { FeatureCollection, Row } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import { ROWS } from "../fixtures";
import { FeatureMap, rowsOf } from "./FeatureMap";

// The map itself is EntityMap's, tested there; here only which rows it is handed.
vi.mock("./EntityMap", () => ({
  EntityMap: ({ rows, location }: { rows: Row[]; location: string }) => (
    <output data-testid="map">{JSON.stringify(rows.map((r) => [r.id, (r[location] as { type?: string } | null)?.type ?? null, r.bikes ?? null]))}</output>
  ),
}));

const STATIONS = ROWS.filter((r) => r.type === "Station");
const OGC = (request: { path: string }) => request.path.includes("/ogc/features/");

function drawn(): unknown {
  return JSON.parse(screen.getByTestId("map").textContent ?? "null");
}

describe("FeatureMap", () => {
  it("draws the Features the endpoint's /items answers, read once with the type as the collection", async () => {
    const client = stubClient({ entities: ROWS });
    render(
      <JcProvider client={client}>
        <FeatureMap type="Station" fallback={[]} />
      </JcProvider>,
    );
    await waitFor(() => expect(drawn()).toHaveLength(3));
    expect(drawn()).toEqual(STATIONS.map((r) => [r.id, "Point", r.bikes]));
    expect(client.transport.calls.filter(OGC).map((c) => c.path)).toEqual(["/api/endpoint/demo/ogc/features/collections/Station/items?limit=1000"]);
  });

  it("draws the rows already read when the endpoint serves no OGC Features, and shows no error", async () => {
    const client = stubClient({
      entities: ROWS,
      refuse: (request) => (OGC(request) ? { status: 404, body: { title: "Not Found", status: 404 } } : null),
    });
    render(
      <JcProvider client={client}>
        <FeatureMap type="Station" fallback={STATIONS.slice(0, 1)} />
      </JcProvider>,
    );
    await waitFor(() => expect(client.transport.calls.some(OGC)).toBe(true));
    expect(drawn()).toEqual([[STATIONS[0].id, "Point", 4]]);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(document.body.textContent).not.toMatch(/Not Found/);
  });

  it("says why when the endpoint refuses the read, and keeps the rows it has on the map", async () => {
    const client = stubClient({
      entities: ROWS,
      refuse: (request) => (OGC(request) ? { status: 403, body: { title: "Forbidden by the endpoint's Policy", status: 403 } } : null),
    });
    render(
      <JcProvider client={client}>
        <FeatureMap type="Station" fallback={STATIONS} />
      </JcProvider>,
    );
    await screen.findByText(/Forbidden by the endpoint's Policy/);
    expect(drawn()).toHaveLength(3);
  });
});

describe("rowsOf", () => {
  it("puts the geometry under the page's location attribute and never lets a property rename the entity", () => {
    const page: FeatureCollection = {
      type: "FeatureCollection",
      features: [
        { type: "Feature", id: "urn:ngsi-ld:Station:1", geometry: { type: "Point", coordinates: [1, 2] }, properties: { id: "x", type: "Y", bikes: 2 } },
        { type: "Feature", id: "urn:ngsi-ld:Station:2", geometry: null, properties: {} },
      ],
    };
    expect(rowsOf("Station", page, "position")).toEqual([
      { id: "urn:ngsi-ld:Station:1", type: "Station", bikes: 2, position: { type: "Point", coordinates: [1, 2] } },
      { id: "urn:ngsi-ld:Station:2", type: "Station", position: null },
    ]);
  });
});
