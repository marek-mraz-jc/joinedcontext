import { describe, expect, it } from "vitest";
import type { Row } from "../src/ngsi";
import { fakeContext, stubClient, stubTransport } from "../src/sdk/testing";

const SAMPLE_ROWS: Row[] = [
  { id: "urn:1", type: "Station", name: "Kamppi", bikes: 5 },
  { id: "urn:2", type: "Station", name: "Kallio", bikes: 2 },
];

describe("sdk testing fixtures", () => {
  it("stubTransport create -> list decodes the Property body and rows() reflects writes", async () => {
    const client = stubClient({ entities: SAMPLE_ROWS });

    const initial = await client.entities.list("Station");
    expect(initial).toHaveLength(2);

    const newId = await client.entities.create(
      "Station",
      {
        name: "Kaivopuisto",
        bikes: 14,
        location: { type: "Point", coordinates: [24.95, 60.15] },
      },
      "003",
    );

    expect(newId).toBe("urn:ngsi-ld:Station:example.org:demo:003");

    const updatedList = await client.entities.list("Station");
    expect(updatedList).toHaveLength(3);

    const created = updatedList.find((r) => r.id === newId);
    expect(created?.name).toBe("Kaivopuisto");
    expect(created?.bikes).toBe(14);
    expect(created?.location).toEqual({ type: "Point", coordinates: [24.95, 60.15] });

    expect(client.transport.rows()).toHaveLength(3);
  });

  it("stubTransport answers a temporal read with the fixture's bodies of that type, and none by default", async () => {
    const temporal = [
      { id: "urn:1", type: "Station", bikes: { type: "Property", values: [[5, "2026-09-25T10:00:00Z"], [4, "2026-09-25T10:10:00Z"]] } },
      { id: "urn:9", type: "Other", bikes: { type: "Property", values: [[1, "2026-09-25T10:00:00Z"]] } },
    ];
    const query = { timerel: "after" as const, timeAt: "2026-09-25T00:00:00Z" };
    const rows = await stubClient({ entities: SAMPLE_ROWS, temporal }).temporal.list("Station", query);
    expect(rows).toEqual([
      {
        id: "urn:1",
        type: "Station",
        series: {
          bikes: [
            { value: 5, observedAt: "2026-09-25T10:00:00Z" },
            { value: 4, observedAt: "2026-09-25T10:10:00Z" },
          ],
        },
      },
    ]);
    expect(await stubClient({ entities: SAMPLE_ROWS }).temporal.list("Station", query)).toEqual([]);
  });

  it("stubTransport returns 404 problem for unknown function", async () => {
    const transport = stubTransport({
      functions: {
        known: () => ({ status: "ok" }),
      },
    });

    const ok = await transport({ method: "POST", path: "/functions/known" });
    expect(ok.status).toBe(200);

    const missing = await transport({ method: "POST", path: "/functions/unknown" });
    expect(missing.status).toBe(404);
  });

  it("fakeContext provides data client, reads rows, and collects log calls", async () => {
    const ctx = fakeContext({
      entities: SAMPLE_ROWS,
      user: { id: "u-123", roles: ["admin"] },
    });

    ctx.log("Starting handler", 1, 2);
    expect(ctx.logs).toEqual([["Starting handler", 1, 2]]);

    expect(ctx.jc.me()?.id).toBe("u-123");
    const rows = await ctx.jc.entities.list("Station");
    expect(rows).toHaveLength(2);
  });
});
