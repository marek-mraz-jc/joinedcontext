import { describe, expect, it } from "vitest";
import { addDays, byStartDay, dayKey, monthGrid, onDay, spans, startOfWeek, timeline, type Span } from "./calendar";
import { events } from "./fixtures";

// Friday 25 September 2026, noon, local time.
const NOW = new Date(2026, 8, 25, 12, 0);
const TODAY = new Date(2026, 8, 25);
const all = spans(events(TODAY));
const names = (list: Span[]) => list.map((span) => span.row.name);

describe("the calendar", () => {
  it("starts a week on Monday, a Sunday included", () => {
    expect(dayKey(startOfWeek(new Date(2026, 8, 27)))).toBe("2026-09-21");
    expect(dayKey(startOfWeek(new Date(2026, 8, 21)))).toBe("2026-09-21");
  });

  it("lays a month out in whole weeks, and only the weeks it needs", () => {
    const september = monthGrid(new Date(2026, 8, 15));
    expect(september).toHaveLength(5);
    expect(dayKey(september[0][0])).toBe("2026-08-31");
    expect(dayKey(september[4][6])).toBe("2026-10-04");
    // February 2027 starts on a Monday and ends on a Sunday.
    expect(monthGrid(new Date(2027, 1, 1))).toHaveLength(4);
  });

  it("leaves out an event with no readable start and reads an end before the start as none", () => {
    const list = spans([
      ...events(TODAY),
      { id: "urn:a", type: "Event", name: "Garbled", startDate: "not a date" },
      { id: "urn:b", type: "Event", name: "Backwards", startDate: "2026-09-25T10:00:00Z", endDate: "2026-09-24T10:00:00Z" },
    ]);
    expect(list).toHaveLength(all.length + 1);
    expect(names(list)).not.toContain("Open studios weekend");
    const backwards = list.find((span) => span.row.name === "Backwards")!;
    expect(backwards.end).toEqual(backwards.start);
    expect(list.map((span) => span.start.getTime())).toEqual([...list.map((span) => span.start.getTime())].sort((a, b) => a - b));
  });

  it("puts a festival on each of its days and on no other", () => {
    for (const offset of [5, 6, 7]) expect(names(onDay(all, addDays(TODAY, offset)))).toEqual(["Design week"]);
    expect(onDay(all, addDays(TODAY, 4))).toEqual([]);
    expect(onDay(all, addDays(TODAY, 8))).toEqual([]);
    expect(names(onDay(all, TODAY))).toEqual(["Farmers' market", "City council open session"]);
  });

  it("keeps what is running now among what is coming, and lists the past latest first", () => {
    const { upcoming, earlier } = timeline(all, NOW);
    expect(upcoming[0].row.name).toBe("Farmers' market");
    expect(names(earlier)).toEqual(["Library reading night", "Harbour clean-up"]);
    expect(timeline([], NOW)).toEqual({ upcoming: [], earlier: [] });
  });

  it("groups the timeline by the day an event starts", () => {
    const groups = byStartDay(timeline(all, NOW).upcoming);
    expect(dayKey(groups[0].day)).toBe("2026-09-25");
    expect(names(groups[0].spans)).toEqual(["Farmers' market", "City council open session"]);
    expect(groups).toHaveLength(7);
  });
});
