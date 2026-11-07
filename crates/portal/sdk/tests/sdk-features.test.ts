import { describe, expect, it } from "vitest";
import { createClient, isEndpointPath, ProblemError } from "../src/sdk/client";
import type { Row } from "../src/ngsi";
import type { JcConfig } from "../src/sdk/config";
import { stubClient } from "../src/sdk/testing";
import type { JcRequest, JcResponse, Transport } from "../src/sdk/transport";

// EP-29 / T-3751: the endpoint's OGC API - Features surface through the gateway path.
const CONFIG: JcConfig = {
  slug: "transport1",
  orgDomain: "hel.fi",
  space: "transportation",
  transport: "bridge",
  endpointName: "transportation",
  endpoints: [
    { name: "transportation", slug: "transport1", space: "transportation", types: ["Vehicle", "Alert"] },
    { name: "transportation-kpis", slug: "kpis1", space: "transportation-kpi", types: ["KeyPerformanceIndicator", "Alert"] },
  ],
};

const FC = { type: "FeatureCollection", features: [], numberMatched: 0, numberReturned: 0 };

function answering(answer: JcResponse): { calls: JcRequest[]; transport: Transport } {
  const calls: JcRequest[] = [];
  const transport: Transport = async (req) => {
    calls.push(req);
    return answer;
  };
  return { calls, transport };
}

/** The query of the one call made, decoded. */
function query(calls: JcRequest[]): Record<string, string> {
  return Object.fromEntries(new URLSearchParams(calls[0].path.split("?")[1]));
}

describe("jc().features", () => {
  it("reads a type's items from the endpoint that serves it, every parameter on the wire", async () => {
    const { calls, transport } = answering({ status: 200, body: FC });
    const client = createClient(CONFIG, transport);
    const page = await client.features.items("Vehicle", {
      bbox: [24.9, 60.1, 25.1, 60.3],
      datetime: "2026-10-01T00:00:00Z/..",
      filter: "speed > 50 AND name LIKE 'A%'",
      properties: ["speed", "name"],
      sortby: ["-speed"],
      limit: 500,
    });
    expect(page).toEqual(FC);
    expect(calls[0].method).toBe("GET");
    expect(calls[0].path.split("?")[0]).toBe("/api/endpoint/transport1/ogc/features/collections/Vehicle/items");
    expect(query(calls)).toEqual({
      limit: "500",
      bbox: "24.9,60.1,25.1,60.3",
      datetime: "2026-10-01T00:00:00Z/..",
      filter: "speed > 50 AND name LIKE 'A%'",
      "filter-lang": "cql2-text",
      properties: "speed,name",
      sortby: "-speed",
    });
  });

  it("asks for 100 items when no limit is given and sends no parameter it was not given", async () => {
    const { calls, transport } = answering({ status: 200, body: FC });
    await createClient(CONFIG, transport).features.items("KeyPerformanceIndicator");
    expect(calls[0].path).toBe("/api/endpoint/kpis1/ogc/features/collections/KeyPerformanceIndicator/items?limit=100");
  });

  it("needs the endpoint named for a type two endpoints serve", async () => {
    const { calls, transport } = answering({ status: 200, body: FC });
    const client = createClient(CONFIG, transport);
    await expect(client.features.items("Alert")).rejects.toThrow(/served by transportation and transportation-kpis/);
    await client.features.items("Alert", { endpoint: "transportation-kpis" });
    expect(calls.map((c) => c.path.split("/")[3])).toEqual(["kpis1"]);
  });

  it.each([
    [{ limit: 0 }, /limit must be between 1 and 1000/],
    [{ limit: 1001 }, /limit must be between 1 and 1000/],
    [{ limit: 2.5 }, /limit must be between 1 and 1000/],
    [{ bbox: [25.1, 60.1, 24.9] as unknown as [number, number, number, number] }, /bbox must be/],
    [{ bbox: [0, 91, 1, 92] as [number, number, number, number] }, /bbox must be/],
    [{ bbox: [0, 10, 1, 5] as [number, number, number, number] }, /bbox must be/],
    [{ bbox: [0, Number.NaN, 1, 5] as [number, number, number, number] }, /bbox must be/],
    [{ properties: ["speed", "a&b=c"] }, /Invalid property: 'a&b=c'/],
    [{ sortby: ["-"] }, /Invalid sort key/],
  ])("refuses %j before any request", async (q, message) => {
    const { calls, transport } = answering({ status: 200, body: FC });
    await expect(createClient(CONFIG, transport).features.items("Vehicle", q)).rejects.toThrow(message);
    expect(calls).toEqual([]);
  });

  it("refuses a collection that is no type name, so no path segment can be smuggled in", async () => {
    const { calls, transport } = answering({ status: 200, body: FC });
    const client = createClient(CONFIG, transport);
    await expect(client.features.items("../admin")).rejects.toThrow(/Invalid collection/);
    await expect(client.features.get("Vehicle/items", "x")).rejects.toThrow(/Invalid collection/);
    expect(calls).toEqual([]);
  });

  it("reads one Feature by its URN, encoded as one segment", async () => {
    const feature = { type: "Feature", id: "urn:ngsi-ld:Vehicle:hel.fi:transportation:v/1", geometry: null, properties: {} };
    const { calls, transport } = answering({ status: 200, body: feature });
    expect(await createClient(CONFIG, transport).features.get("Vehicle", feature.id)).toEqual(feature);
    expect(calls[0].path).toBe(
      "/api/endpoint/transport1/ogc/features/collections/Vehicle/items/urn%3Angsi-ld%3AVehicle%3Ahel.fi%3Atransportation%3Av%2F1",
    );
  });

  it("lists the collections of the primary endpoint or of the one named", async () => {
    const { calls, transport } = answering({ status: 200, body: { collections: [{ id: "Vehicle" }], links: [] } });
    const client = createClient(CONFIG, transport);
    expect(await client.features.collections()).toEqual([{ id: "Vehicle" }]);
    await client.features.collections({ endpoint: "transportation-kpis" });
    expect(calls.map((c) => c.path)).toEqual([
      "/api/endpoint/transport1/ogc/features/collections",
      "/api/endpoint/kpis1/ogc/features/collections",
    ]);
  });

  it("turns a refusal into a ProblemError with the endpoint's status and title", async () => {
    const { transport } = answering({ status: 400, body: { title: "Unsupported CQL2 operator: S_CROSSES", status: 400 } });
    const err = await createClient(CONFIG, transport)
      .features.items("Vehicle", { filter: "S_CROSSES(location, POINT(1 2))" })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ProblemError);
    expect((err as ProblemError).status).toBe(400);
    expect((err as ProblemError).message).toMatch(/S_CROSSES/);
  });

  it("refuses an answer of the wrong shape instead of handing it on", async () => {
    const list = answering({ status: 200, body: [] });
    await expect(createClient(CONFIG, list.transport).features.items("Vehicle")).rejects.toThrow(/did not answer a FeatureCollection/);
    const feature = answering({ status: 200, body: { type: "FeatureCollection", features: [] } });
    await expect(createClient(CONFIG, feature.transport).features.get("Vehicle", "x")).rejects.toThrow(/did not answer a Feature/);
    const collections = answering({ status: 200, body: { links: [] } });
    await expect(createClient(CONFIG, collections.transport).features.collections()).rejects.toThrow(/collection list/);
  });

  it("allows only the OGC Features paths of the endpoint, not other OGC resources", () => {
    expect(isEndpointPath("s", "/api/endpoint/s/ogc/features/collections")).toBe(true);
    expect(isEndpointPath("s", "/api/endpoint/s/ogc/features/collections/Car/items?limit=1")).toBe(true);
    expect(isEndpointPath("s", "/api/endpoint/s/ogc/features/collections/Car/items/urn%3Ax")).toBe(true);
    expect(isEndpointPath("s", "/api/endpoint/s/ogc/features/collections/Car")).toBe(false);
    expect(isEndpointPath("s", "/api/endpoint/s/ogc/features/conformance")).toBe(false);
    expect(isEndpointPath("s", "/api/endpoint/s/ogc/features/collections/Car/items/a/b")).toBe(false);
    // an open interval's `..` is a query value; `..` in the path is still traversal
    expect(isEndpointPath("s", "/api/endpoint/s/ogc/features/collections/Car/items?datetime=2026-01-01T00%3A00%3A00Z%2F..")).toBe(true);
    expect(isEndpointPath("s", "/api/endpoint/s/ogc/features/collections/../items")).toBe(false);
    expect(isEndpointPath("s", "/api/endpoint/s/ngsi-ld/v1/entities/..")).toBe(false);
    expect(isEndpointPath("s", "/api/endpoint/s/ogc/features/collections/Car/items?x=1#frag")).toBe(false);
  });
});

describe("the stub's OGC answers", () => {
  const rows: Row[] = [
    { id: "urn:ngsi-ld:Car:1", type: "Car", speed: 40, location: { type: "Point", coordinates: [24.95, 60.17] } },
    { id: "urn:ngsi-ld:Car:2", type: "Car", speed: 70, location: { type: "Point", coordinates: [27.0, 62.0] } },
    { id: "urn:ngsi-ld:Car:3", type: "Car", speed: 10 },
    { id: "urn:ngsi-ld:Bus:1", type: "Bus", speed: 30 },
  ];

  it("serves a type's rows as Features, the location as the geometry", async () => {
    const client = stubClient({ entities: rows });
    const page = await client.features.items("Car");
    expect(page.features.map((f) => [f.id, f.geometry?.type ?? null, f.properties])).toEqual([
      ["urn:ngsi-ld:Car:1", "Point", { speed: 40 }],
      ["urn:ngsi-ld:Car:2", "Point", { speed: 70 }],
      ["urn:ngsi-ld:Car:3", null, { speed: 10 }],
    ]);
    expect(await client.features.collections()).toEqual([
      { id: "Car", title: "Car" },
      { id: "Bus", title: "Bus" },
    ]);
    expect((await client.features.get("Car", "urn:ngsi-ld:Car:2")).properties).toEqual({ speed: 70 });
    await expect(client.features.get("Car", "urn:ngsi-ld:Car:9")).rejects.toMatchObject({ status: 404 });
  });

  it("keeps the Features in the bbox, the properties asked and the limit", async () => {
    const client = stubClient({ entities: rows });
    const page = await client.features.items("Car", { bbox: [24, 60, 25, 61], properties: ["speed"], limit: 1 });
    expect(page.features.map((f) => f.id)).toEqual(["urn:ngsi-ld:Car:1"]);
    expect(page.numberMatched).toBe(1);
  });

  it("refuses a CQL2 filter it cannot evaluate rather than answer every Feature", async () => {
    const client = stubClient({ entities: rows });
    await expect(client.features.items("Car", { filter: "speed > 50" })).rejects.toMatchObject({ status: 501 });
  });
});
