import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import { perDayOption } from "./charts";
import { byRegister, filterEvents, inputDay, located, onDay, perDay, registerOf, sourceOf, upcomingQuery, when } from "./events";
import { EVENTS as SENT } from "./fixtures/events";

// The rows as the SDK hands them to the page: a language map already read in English.
const EVENTS: Row[] = SENT.map(
  (row) =>
    Object.fromEntries(
      Object.entries(row).map(([key, value]) => [
        key,
        typeof value === "object" && value !== null && "languageMap" in value ? (value.languageMap as Record<string, string>).en : value,
      ]),
    ) as Row,
);
const ids = (rows: Row[]) => rows.map((row) => row.id.slice(row.id.lastIndexOf(":") + 1));
const bare = (id: string, fields: Record<string, unknown> = {}): Row => ({ id, type: "Event", ...fields }) as Row;

describe("the events", () => {
  it("asks only for the events that have not ended, with an unquoted DateTime", () => {
    expect(upcomingQuery(new Date("2030-10-20T00:00:00.000Z"))).toBe("endDate>=2030-10-20T00:00:00Z");
  });

  it("names the register from the local id's prefix, the raw prefix when unknown, Other when none", () => {
    expect(EVENTS.map(registerOf)).toEqual([
      "City of Helsinki",
      "City of Helsinki",
      "City of Espoo",
      "Culture centres",
      "Culture centres",
      "City of Helsinki",
    ]);
    // T-3128: the two registers dev's feed carried as bare codes, named by their Linked Events
    // publishers (ahjo:u4804001020 Kaupunginmuseo, ahjo:u021200 Elinkeino-osasto).
    expect(registerOf(bare("urn:ngsi-ld:Event:hel.fi:helsinki:hkm-23821"))).toBe("Helsinki City Museum");
    expect(registerOf(bare("urn:ngsi-ld:Event:hel.fi:helsinki:elo-004090b6"))).toBe("Economic Development Division");
    expect(registerOf(bare("urn:ngsi-ld:Event:hel.fi:helsinki:vantaa-12"))).toBe("vantaa");
    expect(registerOf(bare("urn:ngsi-ld:Event:hel.fi:helsinki:12"))).toBe("Other");
    expect(registerOf(bare("urn:ngsi-ld:Event:hel.fi:helsinki:-12"))).toBe("Other");
  });

  it("keeps the events overlapping the range, soonest first, and the one with no date", () => {
    const from = inputDay("2030-10-21");
    expect(ids(filterEvents(EVENTS, { from }))).toEqual(["espoo_le-agn5", "kulke-7712", "kulke-7711"]);
    expect(ids(filterEvents(EVENTS, { from: inputDay("2030-10-20"), to: inputDay("2030-10-20", true) }))).toEqual([
      "helsinki-agf1",
      "helsinki-agf2",
    ]);
    const undated = bare("urn:ngsi-ld:Event:hel.fi:helsinki:helsinki-x", { name: "Undated" });
    expect(ids(filterEvents([undated, ...EVENTS], { from }))).toEqual(["espoo_le-agn5", "kulke-7712", "kulke-7711", "helsinki-x"]);
  });

  it("searches every word in the name, place and description, ignoring case and accents", () => {
    expect(ids(filterEvents(EVENTS, { query: "JAZZ stoa" }))).toEqual(["kulke-7711"]);
    expect(ids(filterEvents(EVENTS, { query: "siltakatu" }))).toEqual(["helsinki-agf1"]);
    expect(ids(filterEvents(EVENTS, { query: "   " }))).toHaveLength(EVENTS.length);
    expect(filterEvents(EVENTS, { query: "jazz nowhere" })).toEqual([]);
    const accented = bare("urn:ngsi-ld:Event:hel.fi:helsinki:helsinki-y", { name: "Kesäjuhla" });
    expect(ids(filterEvents([accented], { query: "kesajuhla" }))).toEqual(["helsinki-y"]);
  });

  it("picks one Helsinki day and one register", () => {
    // 22:30 UTC on the 19th is already the 20th in Helsinki.
    const late = bare("urn:ngsi-ld:Event:hel.fi:helsinki:helsinki-late", { startDate: "2030-10-19T22:30:00Z" });
    expect(ids(filterEvents([late, ...EVENTS], { day: "2030-10-20" }))).toEqual(["helsinki-late", "helsinki-agf1", "helsinki-agf2"]);
    expect(ids(filterEvents(EVENTS, { register: "Culture centres" }))).toEqual(["kulke-7712", "kulke-7711"]);
    expect(filterEvents([], { day: "2030-10-20", register: "City of Espoo" })).toEqual([]);
  });

  it("counts the events per day over 30 days, empty days included", () => {
    const series = perDay(EVENTS, new Date("2030-10-19T21:00:00Z"));
    expect(series).toHaveLength(30);
    expect(series[0]).toEqual({ day: "2030-10-20", count: 2 });
    expect(series.slice(1, 4).map((point) => point.count)).toEqual([0, 1, 1]);
    // 5 November falls inside the 30 days; the 2020 event does not.
    expect(series.reduce((sum, point) => sum + point.count, 0)).toBe(5);
    expect(perDay([], new Date("2030-10-19T21:00:00Z"), 0)).toEqual([]);
  });

  // T-3029: the live register is mostly long-running events that opened years ago.
  it("counts an event on every day it takes place, not only the day it started", () => {
    const running = bare("urn:ngsi-ld:Event:hel.fi:helsinki:helsinki-town", {
      startDate: "2001-01-01T08:00:00Z",
      endDate: "2050-12-31T20:00:00Z",
    });
    const series = perDay([running, ...EVENTS], new Date("2030-10-19T21:00:00Z"));
    expect(series.every((point) => point.count >= 1)).toBe(true);
    // The one-day events still count on their own day only.
    expect(series.slice(0, 4).map((point) => point.count)).toEqual([3, 1, 2, 2]);
    // Ends on the 21st in Helsinki (00:30 local on the 21st), so two days, not three.
    const closing = bare("x", { startDate: "2020-01-01T10:00:00Z", endDate: "2030-10-20T21:30:00Z" });
    expect(perDay([closing], new Date("2030-10-19T21:00:00Z"), 3).map((point) => point.count)).toEqual([1, 1, 0]);
    // One date only lasts that day; no date or an end before the start is on no day.
    const once = bare("x", { endDate: "2030-10-21T10:00:00Z" });
    const backwards = bare("x", { startDate: "2030-10-22T10:00:00Z", endDate: "2030-10-20T10:00:00Z" });
    expect(perDay([once, backwards, bare("x")], new Date("2030-10-19T21:00:00Z"), 3).map((point) => point.count)).toEqual([0, 1, 0]);
  });

  it("picks the events taking place on a day, long-running ones included", () => {
    const running = bare("urn:ngsi-ld:Event:hel.fi:helsinki:helsinki-town", {
      startDate: "2001-01-01T08:00:00Z",
      endDate: "2050-12-31T20:00:00Z",
    });
    expect(onDay(running, "2030-10-22")).toBe(true);
    expect(onDay(running, "2051-01-01")).toBe(false);
    expect(ids(filterEvents([running, ...EVENTS], { day: "2030-10-22" }))).toEqual(["helsinki-town", "espoo_le-agn5"]);
  });

  it("draws no per-day chart when no day of the window has an event, so the card can say so", () => {
    const empty = perDay([], new Date("2030-10-19T21:00:00Z"));
    expect(perDayOption(empty)).toBeNull();
    expect(perDayOption([])).toBeNull();
    expect(perDayOption(perDay(EVENTS, new Date("2030-10-19T21:00:00Z")))).not.toBeNull();
  });

  it("counts per register, most first and ties by name", () => {
    expect(byRegister(EVENTS)).toEqual([
      { register: "City of Helsinki", count: 3 },
      { register: "Culture centres", count: 2 },
      { register: "City of Espoo", count: 1 },
    ]);
    expect(byRegister([])).toEqual([]);
  });

  it("links a source only when it is https", () => {
    expect(EVENTS.map(sourceOf).filter(Boolean)).toHaveLength(3);
    expect(sourceOf(EVENTS[2])).toBe("");
    expect(sourceOf(bare("x", { source: "javascript:alert(1)" }))).toBe("");
    expect(sourceOf(bare("x"))).toBe("");
  });

  it("says when in Helsinki's time, and what is missing", () => {
    expect(when(EVENTS[0])).toBe("20 Oct 2030, 17:00 – 20 Oct 2030, 19:00");
    expect(when(bare("x", { startDate: "2030-10-20T14:00:00Z" }))).toBe("from 20 Oct 2030, 17:00");
    expect(when(bare("x", { endDate: "2030-10-20T14:00:00Z" }))).toBe("until 20 Oct 2030, 17:00");
    expect(when(bare("x", { startDate: "not a date" }))).toBe("date not given");
  });

  it("puts only the events with a point on the map", () => {
    expect(EVENTS.filter(located)).toHaveLength(5);
  });
});
