/**
 * Where a run's time went (T-2771, AG-72): the model, each tool and the person, read from the
 * timestamped events alone, so a slow answer is traced to its step.
 */
import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { RunTimeSpent, duration, timeSpent } from "../src/pages/apps/RunTimeSpent";
import type { RunEvent } from "../src/pages/apps/useAgentRun";

let seq = 0;
function event(kind: string, timestamp: string | undefined, payload: Record<string, unknown> = {}): RunEvent {
  seq += 1;
  return { seq, kind, payload: timestamp === undefined ? payload : { ...payload, timestamp } };
}

const RUN: RunEvent[] = [
  event("status", "2026-09-25T08:00:00.000Z", { status: "interviewing" }),
  event("usage", "2026-09-25T08:00:02.100Z", { latencyMs: 2100, model: "google/gemini-3.8-flash", inputTokens: 3980 }),
  event("tool", "2026-09-25T08:00:02.900Z", { tool: "query_endpoint", status: "ok", durationMs: 800 }),
  event("question", "2026-09-25T08:00:03.000Z", { questionId: "q-1" }),
  event("answer", "2026-09-25T08:01:23.000Z", { questionId: "q-1" }),
  event("tool", "2026-09-25T08:01:24.000Z", { tool: "change_resource", status: "failed", durationMs: 1000 }),
  event("usage", "2026-09-25T08:01:30.000Z", { latencyMs: 6000 }),
  event("thought", "2026-09-25T08:01:30.100Z", { text: "Done." }),
];

describe("timeSpent", () => {
  it("splits the run into the model, the tools and the person, every step at its start", () => {
    const spent = timeSpent(RUN);
    expect(spent).toMatchObject({ model: 8100, tools: 1800, waiting: 80_000 });
    expect(spent.steps.map((step) => [step.kind, step.name, step.at, step.ms, step.failed])).toEqual([
      ["model", "google/gemini-3.8-flash", 2100, 2100, false],
      ["tool", "query_endpoint", 2900, 800, false],
      ["waiting", "", 3000, 80_000, false],
      ["tool", "change_resource", 84_000, 1000, true],
      ["model", "", 90_000, 6000, false],
    ]);
  });

  it("counts an event from before timestamps without a start, and skips a waiting it cannot measure", () => {
    const spent = timeSpent([
      event("usage", undefined, { latencyMs: 1500 }),
      event("question", undefined, { questionId: "q" }),
      event("answer", undefined, { questionId: "q" }),
      event("tool", "not a date", { tool: "search_catalog", durationMs: -5 }),
    ]);
    expect(spent).toMatchObject({ model: 1500, tools: 0, waiting: 0 });
    expect(spent.steps).toEqual([{ at: null, kind: "model", name: "", ms: 1500, failed: false }]);
  });

  it("is empty for no events and for events that carry no timing", () => {
    expect(timeSpent([]).steps).toEqual([]);
    expect(timeSpent([event("thought", "2026-09-25T08:00:00Z", { text: "x" })]).steps).toEqual([]);
  });
});

describe("duration", () => {
  it("reads milliseconds, seconds and minutes", () => {
    expect(duration(840, "en")).toBe("840 ms");
    expect(duration(2100, "en")).toBe("2.1 sec");
    expect(duration(80_000, "en")).toBe("1 min 20 sec");
  });
});

describe("RunTimeSpent", () => {
  function show(events: RunEvent[]): void {
    render(
      <I18nextProvider i18n={i18n}>
        <RunTimeSpent events={events} />
      </I18nextProvider>,
    );
  }

  it("names the totals, the slowest step and every step in a table", () => {
    show(RUN);
    const section = screen.getByRole("region", { name: en.agentRun.timeSpent.title });
    expect(within(section).getByText(en.agentRun.timeSpent.waiting).nextSibling).toHaveTextContent("1 min 20 sec");
    expect(
      within(section).getByText(
        en.agentRun.timeSpent.slowest
          .replace("{step}", en.agentRun.timeSpent.waitingStep)
          .replace("{took}", "1 min 20 sec"),
      ),
    ).toBeInTheDocument();
    const table = within(section).getByRole("table", { name: en.agentRun.timeSpent.caption });
    const rows = within(table).getAllByRole("row");
    expect(rows).toHaveLength(6);
    expect(rows[1]).toHaveTextContent("+0:02");
    expect(rows[1]).toHaveTextContent(en.agentRun.timeSpent.modelCallOf.replace("{model}", "google/gemini-3.8-flash"));
    expect(rows[4]).toHaveTextContent("change_resource");
    expect(within(rows[4]).getByText(en.agentRun.timeSpent.failed)).toBeInTheDocument();
  });

  it("draws nothing for a run that recorded no timings", () => {
    const { container } = render(
      <I18nextProvider i18n={i18n}>
        <RunTimeSpent events={[event("thought", "2026-09-25T08:00:00Z", { text: "x" })]} />
      </I18nextProvider>,
    );
    expect(container).toBeEmptyDOMElement();
  });
});
