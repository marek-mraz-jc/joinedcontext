import { describe, expect, it } from "vitest";
import { fakeContext } from "@joinedcontext/sdk/testing";
import type { Row } from "@joinedcontext/sdk/server";
import summary from "./summary";

const ENTITIES: Row[] = [
  { id: "urn:ngsi-ld:Station:example.org:demo:1", type: "Station", name: "Kamppi", bikes: 4, status: "open" },
  { id: "urn:ngsi-ld:Station:example.org:demo:2", type: "Station", name: "Kallio", bikes: 7, status: "closed" },
  { id: "urn:ngsi-ld:Station:example.org:demo:3", type: "Station", name: "Pasila", bikes: null, status: "open" },
  { id: "urn:ngsi-ld:Note:example.org:demo:1", type: "Note", text: "check lock" },
];

describe("summary", () => {
  it("counts every type and averages its numeric attributes", async () => {
    const ctx = fakeContext({ entities: ENTITIES });

    const res = await summary({ method: "POST", query: {}, body: { types: ["Station", "Note", "Empty"] }, user: null }, ctx);

    expect(res.status).toBe(200);
    expect(res.body).toEqual({
      types: [
        { type: "Station", count: 3, averages: { bikes: 5.5 } },
        { type: "Note", count: 1, averages: {} },
        { type: "Empty", count: 0, averages: {} },
      ],
    });
    expect(ctx.logs).toEqual([["summary", 3, "types"]]);
  });

  it("reads the types from the query string on GET", async () => {
    const res = await summary({ method: "GET", query: { types: "Note" }, body: null, user: null }, fakeContext({ entities: ENTITIES }));

    expect(res.body).toEqual({ types: [{ type: "Note", count: 1, averages: {} }] });
  });

  it("reads from the endpoint the page names, and refuses one the application does not read", async () => {
    const ctx = fakeContext({ entities: ENTITIES });

    const named = await summary({ method: "POST", query: {}, body: { types: ["Note"], endpoint: "demo" }, user: null }, ctx);
    expect(named.body).toEqual({ types: [{ type: "Note", count: 1, averages: {} }] });

    await expect(
      summary({ method: "POST", query: {}, body: { types: ["Note"], endpoint: "elsewhere" }, user: null }, ctx),
    ).rejects.toThrow(/Unknown endpoint 'elsewhere'/);
  });

  it.each([[""], [7]])("refuses endpoint %j with 400", async (endpoint) => {
    const res = await summary({ method: "POST", query: {}, body: { types: ["Note"], endpoint }, user: null }, fakeContext({ entities: ENTITIES }));

    expect(res.status).toBe(400);
  });

  it.each([[undefined], [[]], [["Station", "bad type"]], [Array.from({ length: 21 }, (_, i) => `T${i}`)], ["Station"]])(
    "refuses types %j with 400 before reading anything",
    async (types) => {
      const ctx = fakeContext({ entities: ENTITIES });

      const res = await summary({ method: "POST", query: {}, body: { types }, user: null }, ctx);

      expect(res.status).toBe(400);
      expect(ctx.logs).toEqual([]);
    },
  );
});
