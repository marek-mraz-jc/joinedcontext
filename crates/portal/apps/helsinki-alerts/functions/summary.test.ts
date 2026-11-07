import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fakeContext } from "@joinedcontext/sdk/testing";
import { ALERTS } from "../src/fixtures/alerts";
import summary from "./summary";

const request = (roles: string[] | null) => ({
  method: "GET" as const,
  query: {},
  body: null,
  user: roles ? { id: "u1", roles } : null,
});

describe("summary", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-22T09:00:00Z"));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  // AP-40, SDK-21: counts per category and subCategory, and the open alert issued first.
  it("counts the alerts and names the oldest open one for a viewer, with no steward field", async () => {
    const ctx = fakeContext({ entities: ALERTS });

    const res = await summary(request(["viewer"]), ctx);

    expect(res.status).toBe(200);
    expect(res.body).toEqual({
      total: 5,
      byCategory: { traffic: 4, event: 1 },
      bySubCategory: { roadWorks: 3, carAccident: 1, MARKET: 1 },
      // GUID50002 ends at 12:00 today and is still open; GUID50004 ended on the 14th.
      oldestOpen: { id: "urn:ngsi-ld:Alert:hel.fi:helsinki:GUID50002", name: "Ring I lane closure", dateIssued: "2026-09-18T08:00:00Z" },
    });
    expect(res.body).not.toHaveProperty("ownRecords");
    expect(ctx.logs).toEqual([["summary", 5, "alerts"]]);
  });

  // AP-93: the steward's number is there for the steward only.
  it("adds the count of the steward's own records for a steward", async () => {
    const res = await summary(request(["viewer", "steward"]), fakeContext({ entities: ALERTS }));

    expect(res.body).toMatchObject({ total: 5, ownRecords: 1 });
  });

  // An alert with no category is counted, as unknown; a name that is no text is no name.
  it("counts an alert with no category as unknown, and gives a name that is no text as none", async () => {
    const odd = { ...ALERTS[1], category: "", subCategory: undefined, name: 42 } as unknown as (typeof ALERTS)[number];
    const res = await summary(request(["viewer"]), fakeContext({ entities: [odd] }));

    expect(res.body).toMatchObject({
      byCategory: { unknown: 1 },
      bySubCategory: { unknown: 1 },
      oldestOpen: { id: "urn:ngsi-ld:Alert:hel.fi:helsinki:GUID50002", name: null },
    });
  });

  it("gives an anonymous caller no steward field, and an empty space no oldest alert", async () => {
    const res = await summary(request(null), fakeContext({ entities: [] }));

    expect(res.body).toEqual({ total: 0, byCategory: {}, bySubCategory: {}, oldestOpen: null });
  });
});
