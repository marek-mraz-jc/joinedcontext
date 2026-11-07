import { describe, expect, it } from "vitest";
import { createClient, endpointsOf, ProblemError, resolveEndpoint } from "../src/sdk/client";
import { ConfigError, parseConfig } from "../src/sdk/config";
import type { JcConfig } from "../src/sdk/config";
import type { JcRequest, JcResponse, Transport } from "../src/sdk/transport";

// SDK-02: an application of several endpoints, e.g. transportation data and its KPIs.
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

function recording(): { calls: JcRequest[]; transport: Transport } {
  const calls: JcRequest[] = [];
  const transport: Transport = async (req: JcRequest): Promise<JcResponse> => {
    calls.push(req);
    if (req.path.includes("/schema/index.json")) {
      return { status: 200, body: { models: [{ version: 1 }] } };
    }
    if (req.path.includes("/json-schema")) {
      const kpis = req.path.includes("/kpis1/");
      return { status: 200, body: { definitions: kpis ? { KeyPerformanceIndicator: { type: "object" } } : { Vehicle: { type: "object" } } } };
    }
    if (req.path.endsWith("/access")) {
      return { status: 200, body: { grants: [] } };
    }
    return { status: req.method === "GET" ? 200 : 201, body: req.method === "GET" && !req.path.includes("/entities/") ? [] : {} };
  };
  return { calls, transport };
}

describe("an application of several endpoints", () => {
  it("reads a type from the one endpoint that serves it, with no endpoint named", async () => {
    const { calls, transport } = recording();
    const client = createClient(CONFIG, transport);
    await client.entities.list("KeyPerformanceIndicator");
    await client.entities.list("Vehicle");
    await client.temporal.list("KeyPerformanceIndicator", { timerel: "after", timeAt: "2026-09-14T00:00:00Z" });
    expect(calls.map((c) => c.path.split("/")[3])).toEqual(["kpis1", "transport1", "kpis1"]);
  });

  it("refuses a type two endpoints serve until the call names one", async () => {
    const { calls, transport } = recording();
    const client = createClient(CONFIG, transport);
    await expect(client.entities.list("Alert")).rejects.toThrow(ProblemError);
    await expect(client.entities.list("Alert")).rejects.toThrow(/transportation and transportation-kpis/);
    await client.entities.list("Alert", { endpoint: "transportation-kpis" });
    expect(calls).toHaveLength(1);
    expect(calls[0].path).toContain("/api/endpoint/kpis1/");
    await expect(client.entities.list("Vehicle", { endpoint: "nowhere" })).rejects.toThrow(/Unknown endpoint 'nowhere'/);
  });

  it("writes through the endpoint of the type and mints the id in its space", async () => {
    const { calls, transport } = recording();
    const client = createClient(CONFIG, transport);
    const id = await client.entities.create("KeyPerformanceIndicator", { name: "bikes" }, "bikes-avg");
    expect(id).toBe("urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:transportation-kpi:bikes-avg");
    expect(calls[0].path).toBe("/api/endpoint/kpis1/ngsi-ld/v1/entities");
    await client.entities.update("urn:ngsi-ld:Alert:hel.fi:transportation-kpi:a1", { level: "high" }, { endpoint: "transportation-kpis" });
    await client.entities.remove("urn:ngsi-ld:Alert:hel.fi:transportation:a2", { endpoint: "transportation" });
    await client.entities.get("urn:ngsi-ld:Vehicle:hel.fi:transportation:v1");
    expect(calls.slice(1).map((c) => c.path.split("/")[3])).toEqual(["kpis1", "transport1", "transport1"]);
    expect(client.entityId("Vehicle", "v9")).toBe("urn:ngsi-ld:Vehicle:hel.fi:transportation:v9");
  });

  // T-3084, ADR-N-041: an entity is its space and its URN; the URN's segments never pick the
  // endpoint. A type two endpoints serve is refused until the call names one, whatever the URN
  // reads, and a named endpoint is used even when the URN names another space.
  it("never picks an endpoint by the space a URN names", async () => {
    const { calls, transport } = recording();
    const client = createClient(CONFIG, transport);
    await expect(client.entities.update("urn:ngsi-ld:Alert:hel.fi:transportation-kpi:a1", { level: "high" })).rejects.toThrow(
      /pass \{ endpoint/,
    );
    await expect(client.entities.remove("urn:ngsi-ld:Alert:hel.fi:transportation:a2")).rejects.toThrow(ProblemError);
    expect(calls).toHaveLength(0);
    await client.entities.remove("urn:ngsi-ld:Alert:hel.fi:transportation:a2", { endpoint: "transportation-kpis" });
    expect(calls[0].path).toContain("/api/endpoint/kpis1/");
    // A Vehicle is served by one endpoint only: a URN naming the KPI space still goes there.
    await client.entities.get("urn:ngsi-ld:Vehicle:hel.fi:transportation-kpi:v1");
    expect(calls[1].path).toContain("/api/endpoint/transport1/");
  });

  it("keeps a whole URN given to create, and refuses one of another type or shape", async () => {
    const { calls, transport } = recording();
    const client = createClient(CONFIG, transport);
    const kept = await client.entities.create("Vehicle", { name: "tram" }, "urn:ngsi-ld:Vehicle:HSL:1007");
    expect(kept).toBe("urn:ngsi-ld:Vehicle:HSL:1007");
    expect(calls[0].body).toMatchObject({ id: "urn:ngsi-ld:Vehicle:HSL:1007", type: "Vehicle" });
    await expect(client.entities.create("Vehicle", {}, "urn:ngsi-ld:Alert:HSL:1")).rejects.toThrow(/names the type it creates: urn:ngsi-ld:Vehicle:/);
    await expect(client.entities.create("Vehicle", {}, "urn:ngsi-ld:Vehicle:has space")).rejects.toThrow(/RFC 8141/);
    await expect(client.entities.create("Vehicle", {}, `urn:ngsi-ld:Vehicle:${"a".repeat(257)}`)).rejects.toThrow(/RFC 8141/);
    await expect(client.entities.create("Vehicle", {}, "has:colon")).rejects.toThrow(/whole urn:ngsi-ld: id/);
    expect(calls).toHaveLength(1);
  });

  // T-3579: a public App's visitor reads its public endpoint but not the steward's write
  // endpoint beside it; that one's 401 no longer costs the visitor the whole schema.
  it("merges the schemas the caller may read and skips an endpoint that refuses them", async () => {
    for (const refused of [401, 403]) {
      const { transport } = recording();
      const guarded: Transport = async (req) =>
        req.path.startsWith("/api/endpoint/kpis1/") ? { status: refused, body: { title: "Authentication Required" } } : transport(req);
      const client = createClient(CONFIG, guarded);
      expect(Object.keys(await client.schema())).toEqual(["Vehicle"]);
      // Asked for by name, the refused endpoint still says why.
      await expect(client.schema("transportation-kpis")).rejects.toMatchObject({ status: refused });
    }
  });

  it("still fails the merged schema when no endpoint may be read, or one is broken", async () => {
    const { transport } = recording();
    const none: Transport = async (req) =>
      req.path.includes("/schema/") ? { status: 401, body: { title: "Authentication Required" } } : transport(req);
    await expect(createClient(CONFIG, none).schema()).rejects.toMatchObject({ status: 401 });
    const broken: Transport = async (req) =>
      req.path.startsWith("/api/endpoint/kpis1/schema/") ? { status: 502, body: { title: "Bad Gateway" } } : transport(req);
    await expect(createClient(CONFIG, broken).schema()).rejects.toMatchObject({ status: 502 });
  });

  it("merges every endpoint's schema and reads one endpoint's access by name", async () => {
    const { calls, transport } = recording();
    const client = createClient(CONFIG, transport);
    const schema = await client.schema();
    expect(Object.keys(schema).sort()).toEqual(["KeyPerformanceIndicator", "Vehicle"]);
    expect(Object.keys(await client.schema("transportation-kpis"))).toEqual(["KeyPerformanceIndicator"]);
    await client.access("transportation-kpis");
    await client.access();
    const accessPaths = calls.filter((c) => c.path.endsWith("/access")).map((c) => c.path);
    expect(accessPaths).toEqual(["/api/endpoint/kpis1/access", "/api/endpoint/transport1/access"]);
  });

  it("keeps a configuration of one endpoint reading exactly as before", async () => {
    const { calls, transport } = recording();
    const one: JcConfig = { slug: "demo", orgDomain: "example.org", space: "demo", transport: "bridge" };
    const client = createClient(one, transport);
    await client.entities.list("Alert");
    expect(calls[0].path.startsWith("/api/endpoint/demo/")).toBe(true);
    expect(endpointsOf(one)).toEqual([{ name: "demo", slug: "demo", space: "demo", types: [] }]);
    expect(resolveEndpoint(endpointsOf(one), "Anything").slug).toBe("demo");
  });

  it("parses the endpoints of the served configuration and names what is wrong", () => {
    expect(parseConfig(CONFIG).endpoints).toHaveLength(2);
    expect(() => parseConfig({ ...CONFIG, endpoints: [{ name: "", slug: "BAD!", space: "a:b", types: [1] }] })).toThrow(ConfigError);
    try {
      parseConfig({ ...CONFIG, endpoints: [{ name: "", slug: "BAD!", space: "a:b", types: [1] }] });
    } catch (err) {
      const message = (err as Error).message;
      expect(message).toContain("endpoints[0].name");
      expect(message).toContain("endpoints[0].slug");
      expect(message).toContain("endpoints[0].space");
      expect(message).toContain("endpoints[0].types");
    }
    expect(parseConfig({ slug: "demo", orgDomain: "o", space: "s", transport: "bridge" })).not.toHaveProperty("endpoints");
  });
});
