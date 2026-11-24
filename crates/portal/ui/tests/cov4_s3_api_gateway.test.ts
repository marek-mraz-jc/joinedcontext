// T-7706: the gateway client turns an error answer into a problem and reads the type list in each shape.
import { afterEach, describe, expect, it, vi } from "vitest";
import { listSpaceTypes, searchEntities } from "../src/api/gateway";

function stub(body: unknown, status = 200): void {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify(body), { status })));
}

afterEach(() => vi.unstubAllGlobals());

describe("gateway client", () => {
  it("rejects a refused search with the problem's message", async () => {
    stub({ title: "Forbidden", detail: "no access to this type" }, 403);
    await expect(searchEntities({ space: "s1", type: "Room" })).rejects.toThrow(/no access/);
  });

  it("rejects a refused search whose body is not JSON", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("<html>", { status: 502 })));
    await expect(searchEntities({ space: "s1", type: "Room" })).rejects.toBeDefined();
  });

  it("rejects a refused type list with the problem", async () => {
    stub({ title: "Forbidden", detail: "forbidden types" }, 403);
    await expect(listSpaceTypes("s1")).rejects.toThrow(/forbidden types/);
  });

  it("rejects a refused type list whose body is not JSON", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("oops", { status: 500 })));
    await expect(listSpaceTypes("s1")).rejects.toBeDefined();
  });

  it("reads a list of names, a typeList and entries with and without a name", async () => {
    stub(["Room", { typeName: "Sensor", attributeNames: ["a", 3, "b"] }, { typeName: "Bare" }, { nope: 1 }, null, { typeName: "" }]);
    expect(await listSpaceTypes("s1")).toEqual([
      { type: "Room", attributes: [] },
      { type: "Sensor", attributes: ["a", "b"] },
      { type: "Bare", attributes: [] },
    ]);
    stub({ typeList: ["X"] });
    expect(await listSpaceTypes("s1")).toEqual([{ type: "X", attributes: [] }]);
    stub({ other: 1 });
    expect(await listSpaceTypes("s1")).toEqual([]);
  });
});
