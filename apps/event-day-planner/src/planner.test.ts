import { afterEach, describe, expect, it, vi } from "vitest";
import { dayBounds, eventsOfDay, planEvents } from "./events";
import { eventRows } from "./fixtures/events";
import { computeDay, planHere, readAnswer } from "./planner";

const SUNDAY = dayBounds("2030-10-20") as [number, number];
const events = planEvents(eventsOfDay(await eventRows(), SUNDAY), SUNDAY);

// The real WebAssembly planner (test-setup.ts loads it), called as the page calls it.
describe("planner", () => {
  it("orders the chosen events, flags the two at once and writes the calendar", async () => {
    const day = await planHere({ events, settings: { chosen: ["helsinki-agf3", "helsinki-agf1", "helsinki-agf2"], now: SUNDAY[0] } });
    expect(day.items[0].id).toBe("helsinki-agf1");
    expect(day.conflicts).toEqual([["helsinki-agf2", "helsinki-agf3"]]);
    expect(day.items.some((item) => item.fit === "late")).toBe(true);
    expect(day.ics).toContain("SUMMARY:Workshop for Families\r\n");
    expect(day.ics).toContain("DTSTART:20301020T070000Z\r\n");
  });

  it("suggests a day when nothing is chosen", async () => {
    const day = await computeDay({ events, settings: {} });
    expect(day.suggested).toBe(true);
    expect(day.items.length).toBeGreaterThan(1);
    expect(day.items.every((item) => item.fit === "ok")).toBe(true);
  });

  it("plans nothing for no event", async () => {
    const day = await planHere({ events: [], settings: {} });
    expect(day.items).toEqual([]);
  });

  it("says in words what the planner could not read", () => {
    expect(() => readAnswer(JSON.stringify({ error: "the events could not be read: x" }))).toThrow("the events could not be read");
  });
});

// Off the page's thread: the worker answers by id, says what it could not plan, and a worker that
// stops fails what it held and is started again for the next day.
describe("computeDay in a worker", () => {
  const started: FakeWorker[] = [];
  class FakeWorker {
    onmessage: ((event: { data: { id: number; day?: unknown; error?: string } }) => void) | null = null;
    onerror: (() => void) | null = null;
    posted: Array<{ id: number }> = [];
    constructor() {
      started.push(this);
    }
    postMessage(message: { id: number }) {
      this.posted.push(message);
    }
  }
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("answers each call by its id, and fails them all when the worker stops", async () => {
    vi.stubGlobal("Worker", FakeWorker);
    const first = computeDay({ events, settings: {} });
    const second = computeDay({ events, settings: {} });
    const worker = started[0];
    const [a, b] = worker.posted;
    const day = { items: [], conflicts: [], walkMinutes: 0, walkKm: 0, suggested: true, ics: "" };
    worker.onmessage?.({ data: { id: 999, day } });
    worker.onmessage?.({ data: { id: a.id, day } });
    await expect(first).resolves.toEqual(day);
    worker.onmessage?.({ data: { id: b.id, error: "the events could not be read: x" } });
    await expect(second).rejects.toThrow("the events could not be read");

    const third = computeDay({ events, settings: {} });
    expect(started).toHaveLength(1);
    worker.onmessage?.({ data: { id: worker.posted[2].id } });
    await expect(third).rejects.toThrow("The planner did not answer.");

    const fourth = computeDay({ events, settings: {} });
    worker.onerror?.();
    await expect(fourth).rejects.toThrow("The planner stopped. Reload the page.");
    const fifth = computeDay({ events, settings: {} });
    expect(started).toHaveLength(2);
    started[1].onerror?.();
    await expect(fifth).rejects.toThrow("The planner stopped.");
  });
});
