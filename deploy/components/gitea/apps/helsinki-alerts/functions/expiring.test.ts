import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fakeContext } from "@joinedcontext/sdk/testing";
import { ALERTS } from "../src/fixtures/alerts";
import expiring from "./expiring";

const post = (hours: unknown) => ({ method: "POST" as const, query: {}, body: { hours }, user: null });

describe("expiring", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-22T09:00:00Z"));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  // AP-40: the alerts ending in the next N hours, soonest first; ended and open-ended ones are not.
  it("lists the alerts that end within the window, soonest first", async () => {
    const ctx = fakeContext({ entities: ALERTS });

    const day = await expiring(post(24), ctx);
    expect(day.status).toBe(200);
    expect(day.body).toEqual({ hours: 24, alerts: [{ id: "urn:ngsi-ld:Alert:hel.fi:helsinki:GUID50002", name: "Ring I lane closure", validTo: "2026-09-22T12:00:00Z" }] });

    const twoDays = await expiring(post(48), ctx);
    expect((twoDays.body as { alerts: Array<{ id: string }> }).alerts.map((alert) => alert.id)).toEqual([
      "urn:ngsi-ld:Alert:hel.fi:helsinki:GUID50002",
      "urn:ngsi-ld:Alert:hel.fi:helsinki:steward-market-day",
    ]);
  });

  it("reads the hours from the query string on GET", async () => {
    const res = await expiring({ method: "GET", query: { hours: "3" }, body: null, user: null }, fakeContext({ entities: ALERTS }));

    expect(res.body).toEqual({ hours: 3, alerts: [{ id: "urn:ngsi-ld:Alert:hel.fi:helsinki:GUID50002", name: "Ring I lane closure", validTo: "2026-09-22T12:00:00Z" }] });
  });

  // AP-40: a window that is not a whole number of hours is refused before anything is read.
  it.each([[undefined], ["soon"], ["12"], [0], [-1], [1.5], [169], [Number.NaN], [null], [[24]]])(
    "refuses hours %j with 400 before reading anything",
    async (hours) => {
      const ctx = fakeContext({ entities: ALERTS });

      const res = await expiring(post(hours), ctx);

      expect(res.status).toBe(400);
      expect(ctx.logs).toEqual([]);
    },
  );

  it("refuses a non-numeric query string", async () => {
    const res = await expiring({ method: "GET", query: { hours: "2h" }, body: null, user: null }, fakeContext({ entities: ALERTS }));

    expect(res.status).toBe(400);
  });
});
