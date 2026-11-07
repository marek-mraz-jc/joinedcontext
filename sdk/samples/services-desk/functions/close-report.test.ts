import { describe, expect, it } from "vitest";
import { fakeContext } from "@joinedcontext/sdk/testing";
import type { JcUser } from "@joinedcontext/sdk/server";
import closeReport from "./close-report";

const ID = "urn:ngsi-ld:DefectReport:hel.fi:defects:d1";
const STEWARD: JcUser = { id: "u-1", name: "Eero Laine", roles: ["steward"] };
const ROWS = [{ id: ID, type: "DefectReport", name: "Loose kerb stone", status: "open" }];

describe("close-report", () => {
  it("closes the report for a steward and nothing else of it", async () => {
    const ctx = fakeContext({ entities: ROWS });
    const res = await closeReport({ method: "POST", query: {}, body: { id: ID }, user: STEWARD }, ctx);
    expect(res).toEqual({ status: 200, body: { id: ID, status: "closed" } });
    expect(await ctx.jc.entities.all("DefectReport")).toEqual([{ ...ROWS[0], status: "closed" }]);
    expect(ctx.logs).toEqual([["closed", ID]]);
  });

  it("answers 403 to a person without the role and 401 to nobody, before reading the body", async () => {
    const ctx = fakeContext({ entities: ROWS });
    const clerk = await closeReport({ method: "POST", query: {}, body: { id: ID }, user: { id: "u-2", roles: ["clerk"] } }, ctx);
    expect(clerk.status).toBe(403);
    expect(clerk.body).toMatchObject({ role: "steward", detail: "this needs the App's role 'steward'" });
    const nobody = await closeReport({ method: "POST", query: {}, body: { id: "not an id" }, user: null }, ctx);
    expect(nobody.status).toBe(401);
    expect(ctx.logs).toEqual([]);
  });

  it("refuses an id that is not one report's", async () => {
    const ctx = fakeContext({ entities: ROWS });
    for (const id of [undefined, "", 7, "urn:ngsi-ld:Alert:x", `${ID}/../x`]) {
      const res = await closeReport({ method: "POST", query: {}, body: { id }, user: STEWARD }, ctx);
      expect(res.status, String(id)).toBe(400);
    }
    expect(ctx.logs).toEqual([]);
  });
});
