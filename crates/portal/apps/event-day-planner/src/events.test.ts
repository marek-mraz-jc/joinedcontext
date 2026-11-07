import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import { eventRows } from "./fixtures/events";
import { dayBounds, during, eventsOfDay, localOf, perHour, planEvents, sourceOf, startHour, timeOf, upcomingQuery, zoneOffset } from "./events";

const SUNDAY = dayBounds("2030-10-20") as [number, number];
const EVENTS = await eventRows();

describe("events", () => {
  it("knows where Helsinki's days begin, summer time and winter time", () => {
    expect(new Date(SUNDAY[0]).toISOString()).toBe("2030-10-19T21:00:00.000Z");
    expect(new Date(SUNDAY[1]).toISOString()).toBe("2030-10-20T21:00:00.000Z");
    // The clocks go back on 27 October 2030: that day has 25 hours.
    const [start, end] = dayBounds("2030-10-27") as [number, number];
    expect((end - start) / 3_600_000).toBe(25);
    expect(zoneOffset(Date.UTC(2030, 0, 1))).toBe(2 * 3_600_000);
    for (const wrong of ["", "2030-02-30", "20.10.2030", "2030-13-01"]) expect(dayBounds(wrong)).toBeNull();
  });

  it("takes the day's events, soonest first, an exhibition open for months among them", () => {
    const ids = eventsOfDay(EVENTS, SUNDAY).map((row) => localOf(row.id));
    expect(ids).toEqual(["hkm-5", "helsinki-agf1", "helsinki-agf2", "helsinki-agf3", "helsinki-agf6", "kulke-4"]);
    expect(eventsOfDay(EVENTS, SUNDAY, "STOA").map((row) => localOf(row.id))).toEqual(["kulke-4"]);
    expect(eventsOfDay(EVENTS, SUNDAY, "", 12).map((row) => localOf(row.id))).toEqual(["helsinki-agf2", "helsinki-agf3"]);
    expect(eventsOfDay([], SUNDAY)).toEqual([]);
  });

  it("counts the events starting each Helsinki hour, an earlier one at midnight", () => {
    const counts = perHour(eventsOfDay(EVENTS, SUNDAY), SUNDAY);
    expect(counts[0]).toBe(1);
    expect(counts[10]).toBe(1);
    expect(counts[12]).toBe(2);
    expect(counts[18]).toBe(1);
    expect(counts.reduce((a, b) => a + b, 0)).toBe(6);
    expect(startHour(EVENTS[0], SUNDAY)).toBe(10);
  });

  it("cuts each window to the day and keeps a missing place as unknown", () => {
    const [museum] = planEvents([EVENTS[4]], SUNDAY);
    expect([museum.start, museum.end]).toEqual(SUNDAY);
    const [undated] = planEvents([{ id: "urn:ngsi-ld:Event:x:y:z", type: "Event" } as Row], SUNDAY);
    expect([undated.start, undated.end, undated.lon, undated.lat, undated.name]).toEqual([null, null, null, null, "z"]);
    expect(during({ id: "a", type: "Event" } as Row, SUNDAY)).toBe(false);
  });

  it("asks only for events that have not ended, and links only an https source", () => {
    expect(upcomingQuery(SUNDAY[0])).toBe("endDate>=2030-10-19T21:00:00Z");
    expect(sourceOf(EVENTS[0])).toBe("https://api.hel.fi/linkedevents/v1/");
    expect(sourceOf({ id: "a", type: "Event", source: "javascript:alert(1)" } as Row)).toBe("");
  });

  // What a feed may send that the day still reads: no end, no start, a date that is no date.
  it("reads an event with no end as a moment, and one with no start or no date as none", () => {
    const at = (fields: Record<string, unknown>) => ({ id: "urn:ngsi-ld:Event:hel.fi:helsinki:x", type: "Event", ...fields }) as Row;
    const moment = at({ startDate: "2030-10-20T09:00:00Z" });
    expect(during(moment, SUNDAY)).toBe(true);
    const [planned] = planEvents([moment], SUNDAY);
    expect(planned.end).toBe(planned.start);
    expect(timeOf(at({ startDate: "soon" }), "startDate")).toBeNull();
    expect(timeOf(at({ startDate: 42 }), "startDate")).toBeNull();
    expect(during(at({ startDate: "soon" }), SUNDAY)).toBe(false);
    // No start: counted in the day's first hour, planned with no time.
    expect(perHour([at({})], SUNDAY)[0]).toBe(1);
    expect(planEvents([at({})], SUNDAY)[0]).toMatchObject({ start: null, end: null, lon: null, lat: null });
  });
});
