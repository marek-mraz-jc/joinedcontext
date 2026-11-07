/** T-3259: a schedule in words, the cron it writes and reads back, and the next runs it makes. */
import { describe, expect, it } from "vitest";
import { choiceOf, cronOf, intervalSeconds, nextRuns, parseCron, scheduleWords } from "../src/pages/pipelines/cron";
import i18n from "../src/i18n";
import type { Choice } from "../src/pages/pipelines/cron";

const iso = (dates: Date[]) => dates.map((date) => date.toISOString().slice(0, 16));

describe("a schedule's next runs", () => {
  const from = new Date("2026-10-07T22:47:30Z");

  it("steps every 15 minutes from the next quarter", () => {
    expect(iso(nextRuns(parseCron("*/15 * * * *")!, from))).toEqual([
      "2026-10-07T23:00", "2026-10-07T23:15", "2026-10-07T23:30", "2026-10-07T23:45", "2026-10-08T00:00",
    ]);
  });

  it("runs daily at 03:00 UTC and weekly on Monday", () => {
    expect(iso(nextRuns(parseCron("0 3 * * *")!, from, 2))).toEqual(["2026-10-08T03:00", "2026-10-09T03:00"]);
    // 2026-10-12 is a Monday.
    expect(iso(nextRuns(parseCron("30 6 * * 1")!, from, 2))).toEqual(["2026-10-12T06:30", "2026-10-19T06:30"]);
  });

  it("reads lists, ranges, steps of a range, Sunday as 0 or 7, and day OR weekday", () => {
    expect(iso(nextRuns(parseCron("0 8-10/2 * * *")!, from, 3))).toEqual(["2026-10-08T08:00", "2026-10-08T10:00", "2026-10-09T08:00"]);
    expect(iso(nextRuns(parseCron("0 0 * * 7")!, from, 1))).toEqual(["2026-10-11T00:00"]);
    // The 15th, or any Friday (the 9th).
    expect(iso(nextRuns(parseCron("0 0 15 * 5")!, from, 2))).toEqual(["2026-10-09T00:00", "2026-10-15T00:00"]);
  });

  it("refuses what the runner would not read, and finds no run for a day that never comes", () => {
    for (const text of ["", "* * * *", "60 * * * *", "* 24 * * *", "*/0 * * * *", "a * * * *", "5-1 * * * *"]) {
      expect(parseCron(text)).toBeUndefined();
    }
    expect(nextRuns(parseCron("0 0 31 2 *")!, from)).toEqual([]);
  });

  it("knows the interval between two starts", () => {
    expect(intervalSeconds(parseCron("*/15 * * * *")!, from)).toBe(900);
    expect(intervalSeconds(parseCron("0 3 * * *")!, from)).toBe(86_400);
  });
});

describe("a schedule in words (PL-69)", () => {
  it("writes the cron of each choice and reads the same choice back", () => {
    const choices: Choice[] = [
      { kind: "minutes", every: 15 },
      { kind: "hourly", minute: 5 },
      { kind: "hours", every: 6, minute: 0 },
      { kind: "daily", hour: 3, minute: 0 },
      { kind: "weekly", weekday: 1, hour: 6, minute: 30 },
    ];
    for (const choice of choices) expect(choiceOf(cronOf(choice)!)).toEqual(choice);
  });

  it("calls anything the choices do not say custom", () => {
    for (const text of ["0 8-10/2 * * *", "*/7 * * * *", "0 0 1 * *", "0 0 * 1 *", "nonsense"]) {
      expect(choiceOf(text)).toEqual({ kind: "custom" });
    }
    expect(choiceOf("0 0 * * 7")).toEqual({ kind: "weekly", weekday: 0, hour: 0, minute: 0 });
  });
});

describe("the pipeline form's schedule", () => {
  it("is asked in words while the pipeline is scheduled, and hidden otherwise", async () => {
    const { pipelineUiSchemaFor } = await import("../src/schemas/kinds");
    const shown = { source: "datasource" as const, readsMore: false, computeFilled: [], scheduled: true };
    expect((pipelineUiSchemaFor(shown) as Record<string, unknown>).schedule).toEqual({ "ui:widget": "cronSchedule" });
    expect((pipelineUiSchemaFor({ ...shown, scheduled: false }) as Record<string, unknown>).schedule).toEqual({ "ui:widget": "hidden" });
  });
});

describe("a pipeline page's schedule", () => {
  const t = i18n.getFixedT("en");
  it("says each choice in words", () => {
    expect(scheduleWords("*/30 * * * *", t, "en")).toBe("Every 30 minutes");
    expect(scheduleWords("5 * * * *", t, "en")).toBe("Every hour, at minute 5");
    expect(scheduleWords("0 */6 * * *", t, "en")).toBe("Every 6 hours, at minute 0");
    expect(scheduleWords("0 3 * * *", t, "en")).toBe("Every day at 03:00 UTC");
    expect(scheduleWords("30 7 * * 1", t, "en")).toBe("Every Monday at 07:30 UTC");
  });

  it("leaves a cron the choices do not say to the caller", () => {
    expect(scheduleWords("0 3 1 * *", t, "en")).toBeUndefined();
    expect(scheduleWords("not a cron", t, "en")).toBeUndefined();
  });
});
