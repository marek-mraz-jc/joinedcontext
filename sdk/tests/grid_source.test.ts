import { describe, expect, it, vi } from "vitest";
import type { JcRequest, JcResponse } from "../src/sdk/transport";
import { endpointSource, spaceSource, fixtureSource, historyOf, orderRows, sourceFor, SourceError } from "../src/grid/source";
import { areaQuery, ringOfBounds } from "../src/grid/geoarea";

function stubTransport(responses: Map<string, (req: JcRequest) => JcResponse>): (req: JcRequest) => Promise<JcResponse> {
  const calls: JcRequest[] = [];
  const fn = async (req: JcRequest): Promise<JcResponse> => {
    calls.push(req);
    const key = `${req.method} ${req.path}`;
    const handler = responses.get(key);
    if (handler) return handler(req);
    return { status: 404, body: { title: "Not Found" } };
  };
  fn.calls = calls;
  return fn as unknown as (req: JcRequest) => Promise<JcResponse> & { calls: JcRequest[] };
}

const sampleEntity: Record<string, unknown> = {
  id: "urn:ngsi-ld:Bike:hel:fi:001",
  type: "Bike",
  createdAt: "2025-01-01T00:00:00Z",
  modifiedAt: "2025-01-02T00:00:00Z",
  availableBikeNumber: {
    type: "Property",
    value: 5,
    unitCode: "C62",
    observedAt: "2025-01-01T12:00:00Z",
  },
  refDevice: {
    type: "Relationship",
    object: "urn:ngsi-ld:Device:hel:fi:d1",
  },
  location: {
    type: "GeoProperty",
    value: { type: "Point", coordinates: [24.95, 60.17] },
  },
};

const sampleEntity2: Record<string, unknown> = {
  id: "urn:ngsi-ld:Bike:hel:fi:002",
  type: "Bike",
  availableBikeNumber: {
    type: "Property",
    value: 3,
    unitCode: "C62",
  },
};

describe("endpointSource", () => {
  it("builds query URL with options=sysAttrs, type, limit, and no id in attrs", async () => {
    const transport = stubTransport(new Map([
      ["GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50", () => ({ status: 200, body: [sampleEntity] })],
    ]));
    const source = endpointSource("demo", transport);
    const result = await source.query({ type: "Bike" }, { offset: 0, limit: 50 });
    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].id).toBe("urn:ngsi-ld:Bike:hel:fi:001");
    const call = (transport as unknown as { calls: JcRequest[] }).calls[0];
    expect(call.path).toContain("options=sysAttrs");
    expect(call.path).toContain("type=Bike");
    expect(call.path).toContain("limit=50");
    // The footer shows how many entities the query matches, and only the broker can say
    // (T-1429): the grid holds one page and a page cannot be counted into a total.
    expect(call.path).toContain("count=true");
    expect(call.path).not.toContain("id");
  });

  it("asks for rows alone when the caller needs no total (T-6161)", async () => {
    const transport = stubTransport(new Map());
    const source = endpointSource("demo", transport);
    await source.query({ type: "Bike" }, { offset: 100, limit: 50, count: false }).catch(() => undefined);
    const call = (transport as unknown as { calls: JcRequest[] }).calls[0];
    // A count scans the whole match on the broker: a next page or a probe skips it.
    expect(call.path).not.toContain("count=");
    expect(call.path).toContain("offset=100");
  });

  it("asks the drawn area as a geo query, all four parameters together (UI-72)", async () => {
    const transport = stubTransport(new Map());
    const source = endpointSource("demo", transport);
    const area = areaQuery({
      geoproperty: "location",
      geometry: { type: "Polygon", coordinates: [ringOfBounds(24.9, 60.1, 25, 60.2)] } as never,
    })!;
    await source.query({ type: "Bike", area }, { offset: 0, limit: 50 }).catch(() => undefined);
    const call = (transport as unknown as { calls: JcRequest[] }).calls[0];
    expect(call.path).toContain("georel=within");
    expect(call.path).toContain("geometry=Polygon");
    expect(call.path).toContain("geoproperty=location");
    // Encoded, because the ring is JSON with brackets and commas in it.
    expect(call.path).toContain(encodeURIComponent(JSON.stringify([ringOfBounds(24.9, 60.1, 25, 60.2)])));
  });

  it("asks no geo parameter at all when nothing is drawn", async () => {
    const transport = stubTransport(new Map());
    const source = endpointSource("demo", transport);
    await source.query({ type: "Bike" }, { offset: 0, limit: 50 }).catch(() => undefined);
    const call = (transport as unknown as { calls: JcRequest[] }).calls[0];
    // `coordinates` without `georel` is ignored by the broker and reads as an empty area, so
    // neither may be sent alone.
    expect(call.path).not.toContain("georel");
    expect(call.path).not.toContain("coordinates");
    expect(call.path).not.toContain("geoproperty");
  });

  it("escapes q parameter correctly", async () => {
    const transport = stubTransport(new Map([
      ["GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50&q=speed%3E10%3Bbrand%3D%3D%22a%26b%22", () => ({ status: 200, body: [sampleEntity] })],
    ]));
    const source = endpointSource("demo", transport);
    await source.query({ type: "Bike", q: 'speed>10;brand=="a&b"' }, { offset: 0, limit: 50 });
    const call = (transport as unknown as { calls: JcRequest[] }).calls[0];
    expect(call.path).toContain("q=speed%3E10%3Bbrand%3D%3D%22a%26b%22");
  });

  it("sends PATCH with JSON body and content type", async () => {
    const transport = stubTransport(new Map([
      ["PATCH /api/endpoint/demo/ngsi-ld/v1/entities/urn%3Angsi-ld%3ABike%3Ahel%3Afi%3A001/attrs", (req) => {
        expect(req.body).toEqual({ availableBikeNumber: { type: "Property", value: 10 } });
        return { status: 204, body: null };
      }],
    ]));
    const source = endpointSource("demo", transport);
    await source.patch!("urn:ngsi-ld:Bike:hel:fi:001", { availableBikeNumber: { type: "Property", value: 10 } });
    const call = (transport as unknown as { calls: JcRequest[] }).calls[0];
    expect(call.method).toBe("PATCH");
    expect(call.body).toEqual({ availableBikeNumber: { type: "Property", value: 10 } });
  });

  it("throws SourceError with detail from problem body on 403", async () => {
    const transport = stubTransport(new Map([
      ["GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50", () => ({
        status: 403,
        body: { type: "about:blank", title: "Forbidden", detail: "You are not allowed to read this type" },
      })],
    ]));
    const source = endpointSource("demo", transport);
    await expect(source.query({ type: "Bike" }, { offset: 0, limit: 50 })).rejects.toThrow(SourceError);
    await expect(source.query({ type: "Bike" }, { offset: 0, limit: 50 })).rejects.toMatchObject({ status: 403, message: "You are not allowed to read this type" });
  });

  it("get returns null on 404", async () => {
    const transport = stubTransport(new Map([
      ["GET /api/endpoint/demo/ngsi-ld/v1/entities/urn%3Ax?options=sysAttrs", () => ({ status: 404, body: null })],
    ]));
    const source = endpointSource("demo", transport);
    const result = await source.get("urn:x");
    expect(result).toBeNull();
  });
});

describe("spaceSource", () => {
  it("uses /cs/{space}/ngsi-ld/v1/entities path", async () => {
    const transport = stubTransport(new Map([
      ["GET /cs/helsinki-bikes/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50", () => ({ status: 200, body: [sampleEntity] })],
    ]));
    const source = spaceSource("helsinki-bikes", transport);
    const result = await source.query({ type: "Bike" }, { offset: 0, limit: 50 });
    expect(result.rows).toHaveLength(1);
    const call = (transport as unknown as { calls: JcRequest[] }).calls[0];
    expect(call.path).toContain("/cs/helsinki-bikes/ngsi-ld/v1/entities");
  });

  it("throws SourceError with message 'no grant on this space' on 404", async () => {
    const transport = stubTransport(new Map([
      ["GET /cs/helsinki-bikes/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50", () => ({ status: 404, body: null })],
    ]));
    const source = spaceSource("helsinki-bikes", transport);
    await expect(source.query({ type: "Bike" }, { offset: 0, limit: 50 })).rejects.toThrow(SourceError);
    await expect(source.query({ type: "Bike" }, { offset: 0, limit: 50 })).rejects.toMatchObject({ status: 404, message: "no grant on this space" });
  });

  it("get returns null on 404", async () => {
    const transport = stubTransport(new Map([
      ["GET /cs/helsinki-bikes/ngsi-ld/v1/entities/urn%3Ax?options=sysAttrs", () => ({ status: 404, body: null })],
    ]));
    const source = spaceSource("helsinki-bikes", transport);
    const result = await source.get("urn:x");
    expect(result).toBeNull();
  });
});

describe("fixtureSource", () => {
  const entities = [sampleEntity, sampleEntity2];

  it("filters by type and pages", async () => {
    const source = fixtureSource(entities);
    const page1 = await source.query({ type: "Bike" }, { offset: 0, limit: 1 });
    expect(page1.rows).toHaveLength(1);
    expect(page1.total).toBe(2);
    const page2 = await source.query({ type: "Bike" }, { offset: 1, limit: 1 });
    expect(page2.rows).toHaveLength(1);
    expect(page2.rows[0].id).toBe("urn:ngsi-ld:Bike:hel:fi:002");
  });

  it("filters by idPattern", async () => {
    const source = fixtureSource(entities);
    const result = await source.query({ type: "Bike", idPattern: "001$" }, { offset: 0, limit: 10 });
    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].id).toBe("urn:ngsi-ld:Bike:hel:fi:001");
  });

  it("get returns entity by id", async () => {
    const source = fixtureSource(entities);
    const entity = await source.get("urn:ngsi-ld:Bike:hel:fi:001");
    expect(entity).not.toBeNull();
    expect(entity!.id).toBe("urn:ngsi-ld:Bike:hel:fi:001");
  });

  it("get returns null for unknown id", async () => {
    const source = fixtureSource(entities);
    const entity = await source.get("urn:unknown");
    expect(entity).toBeNull();
  });
});

describe("sourceFor", () => {
  it("returns endpointSource for endpoint kind", () => {
    const transport = vi.fn() as unknown as (req: JcRequest) => Promise<JcResponse>;
    const source = sourceFor({ kind: "endpoint", slug: "demo" }, transport);
    expect(source.query).toBeDefined();
    expect(source.get).toBeDefined();
  });

  it("returns spaceSource for space kind", () => {
    const transport = vi.fn() as unknown as (req: JcRequest) => Promise<JcResponse>;
    const source = sourceFor({ kind: "space", space: "helsinki" }, transport);
    expect(source.query).toBeDefined();
    expect(source.get).toBeDefined();
  });
});

describe("history", () => {
  it("asks between two instants and reads both temporal shapes", async () => {
    const seen: string[] = [];
    const transport = async (req: JcRequest): Promise<JcResponse> => {
      seen.push(req.path);
      return {
        status: 200,
        body: { speed: [{ type: "Property", value: 3, observedAt: "2026-01-01T00:00:00Z" }] },
      };
    };
    const points = await endpointSource("demo", transport).history!("urn:x", "speed", {
      from: "2026-01-01T00:00:00Z",
      to: "2026-01-02T00:00:00Z",
    });
    expect(seen[0]).toContain("/temporal/entities/urn%3Ax?attrs=speed&timerel=between");
    expect(seen[0]).toContain("endTimeAt=2026-01-02T00%3A00%3A00Z");
    expect(points).toEqual([{ at: "2026-01-01T00:00:00Z", value: 3 }]);
    expect(historyOf({ speed: { values: [[4, "2026-01-01T01:00:00Z"]] } }, "speed")).toEqual([
      { at: "2026-01-01T01:00:00Z", value: 4 },
    ]);
    expect(historyOf(null, "speed")).toEqual([]);
  });

  it("an endpoint's 404 is a history with nothing in it, and a refusal stays a refusal (T-2972)", async () => {
    const answering = (status: number) => async (): Promise<JcResponse> => ({
      status,
      body: { title: "Refused", detail: "Access Denied by Policy" },
    });
    await expect(endpointSource("demo", answering(404)).history!("urn:x", "a", { lastN: 5 })).resolves.toEqual([]);
    await expect(endpointSource("demo", answering(403)).history!("urn:x", "a", { lastN: 5 })).rejects.toMatchObject({
      status: 403,
      message: "Access Denied by Policy",
    });
  });

  it("a space's 404 on a history read is nothing recorded, never a missing grant (T-5597)", async () => {
    const answering = (status: number) => async (): Promise<JcResponse> => ({ status, body: { title: "x" } });
    await expect(spaceSource("s", answering(404)).history!("urn:x", "name", { lastN: 5 })).resolves.toEqual([]);
    await expect(spaceSource("s", answering(403)).history!("urn:x", "a", { lastN: 5 })).rejects.toMatchObject({
      status: 403,
    });
    await expect(spaceSource("s", answering(503)).history!("urn:x", "a", { lastN: 5 })).rejects.toMatchObject({
      status: 503,
    });
    // the query keeps hiding whether the space exists
    await expect(spaceSource("s", answering(404)).query({ type: "T" }, { offset: 0, limit: 5 })).rejects.toMatchObject({
      message: "no grant on this space",
    });
  });
});

describe("the count the footer shows (T-1429, R22)", () => {
  it("is the endpoint's own NGSILD-Results-Count", async () => {
    const transport = stubTransport(new Map([
      [
        "GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50",
        () => ({ status: 200, body: [sampleEntity], headers: { "ngsild-results-count": "412" } }),
      ],
    ]));
    const page = await endpointSource("demo", transport).query({ type: "Bike" }, { offset: 0, limit: 50 });
    expect(page.total).toBe(412);
  });

  it("is absent when the answer carries none, or carries something that is not a count", async () => {
    // A narrowed answer has its count removed on purpose: the difference between the count and
    // the rows is the number of entities the policy withheld (R22).
    const without = stubTransport(new Map([
      [
        "GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50",
        () => ({ status: 200, body: [sampleEntity] }),
      ],
    ]));
    expect((await endpointSource("demo", without).query({ type: "Bike" }, { offset: 0, limit: 50 })).total)
      .toBeUndefined();

    for (const raw of ["", "many", "-1", "1.5"]) {
      const odd = stubTransport(new Map([
        [
          "GET /api/endpoint/demo/ngsi-ld/v1/entities?type=Bike&options=sysAttrs&count=true&limit=50",
          () => ({ status: 200, body: [sampleEntity], headers: { "ngsild-results-count": raw } }),
        ],
      ]));
      expect(
        (await endpointSource("demo", odd).query({ type: "Bike" }, { offset: 0, limit: 50 })).total,
        raw,
      ).toBeUndefined();
    }
  });
});

// The order of the whole answer (T-5082): the endpoint is asked with `orderBy`, and the fixture
// orders as the broker does, per kind of value, an entity without the attribute last either way.
describe("orderBy", () => {
  it("is sent to the endpoint as CIM 009 writes it, and only when asked", async () => {
    const paths: string[] = [];
    const transport = async (req: JcRequest): Promise<JcResponse> => {
      paths.push(req.path);
      return { status: 200, body: [] };
    };
    const source = endpointSource("air", transport);
    expect(source.orders).toBe(true);
    await source.query({ type: "Bike", orderBy: "availableBikeNumber;desc" }, { offset: 0, limit: 5 });
    await source.query({ type: "Bike" }, { offset: 0, limit: 5 });
    expect(new URL(paths[0], "http://x").searchParams.get("orderBy")).toBe("availableBikeNumber;desc");
    expect(paths[1]).not.toContain("orderBy");
  });

  const row = (id: string, attrs: Record<string, unknown>) =>
    fixtureSource([{ id, type: "T", ...attrs }]).get(id) as Promise<NonNullable<Awaited<ReturnType<ReturnType<typeof fixtureSource>["get"]>>>>;

  it("orders text, numbers, dates and missing values as the broker does", async () => {
    const rows = await Promise.all([
      row("urn:ngsi-ld:T:a", { name: { type: "Property", value: "Žilina" }, n: { type: "Property", value: 10 }, at: { type: "Property", value: "2026-03-01T00:00:00Z" } }),
      row("urn:ngsi-ld:T:b", { name: { type: "Property", value: "Banská Bystrica" }, n: { type: "Property", value: 9 }, at: { type: "Property", value: "2025-12-31T23:59:59Z" } }),
      row("urn:ngsi-ld:T:c", {}),
      row("urn:ngsi-ld:T:d", { name: { type: "Property", value: "Košice" }, n: { type: "Property", value: -1 }, at: { type: "Property", value: "2026-01-15T08:00:00Z" } }),
    ]);
    const ids = (orderBy: string) => orderRows(rows, orderBy).map((each) => each.id.slice(-1)).join("");
    // Numbers by value, not by their text: 9 before 10.
    expect(ids("n")).toBe("dbac");
    expect(ids("n;desc")).toBe("abdc");
    expect(ids("name")).toBe("bdac");
    expect(ids("name;desc")).toBe("adbc");
    // ISO 8601 instants order as text, which is their time order.
    expect(ids("at")).toBe("bdac");
    expect(ids("at;desc")).toBe("adbc");
    expect(ids("id;desc")).toBe("dcba");
  });

  it("orders the whole fixture before it pages", async () => {
    const source = fixtureSource([
      { id: "urn:ngsi-ld:T:1", type: "T", n: { type: "Property", value: 2 } },
      { id: "urn:ngsi-ld:T:2", type: "T", n: { type: "Property", value: 1 } },
      { id: "urn:ngsi-ld:T:3", type: "T", n: { type: "Property", value: 3 } },
    ]);
    const page = await source.query({ type: "T", orderBy: "n;desc" }, { offset: 1, limit: 1 });
    expect(page.rows.map((each) => each.id)).toEqual(["urn:ngsi-ld:T:1"]);
    expect(page.total).toBe(3);
  });
});
