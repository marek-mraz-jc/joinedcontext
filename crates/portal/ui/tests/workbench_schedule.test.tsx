/**
 * T-7062 (G-23): the workbench's "When it runs" step. A daily 03:00 load is set with no cron
 * typed, the choice writes the cron and a typed cron reads back as the choice; a schedule faster
 * than the project's minimum (PL-116) is refused at the field with the reason; axe finds nothing.
 */
import { useState } from "react";
import type { JSX } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import axe from "axe-core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { PipelineForm } from "../src/pages/pipelines/PipelineEditor";
import { FIRST_SCHEDULE, WorkbenchSchedule, clockWords, scheduleGap, scheduled } from "../src/pages/pipelines/WorkbenchSchedule";

const w = en.pipelines.workbench.schedule;
let floor: number | undefined;
let seen: PipelineForm[] = [];

beforeEach(async () => {
  await i18n.changeLanguage("en");
  floor = undefined;
  seen = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input instanceof Request ? input : new Request(String(input));
      const path = new URL(request.url, window.location.origin).pathname;
      if (path === "/api/v1/projects/city") {
        return Response.json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Project", metadata: { name: "city" }, spec: floor ? { quotas: { minPipelinePeriodSeconds: floor } } : {} });
      }
      return Response.json({ items: [] });
    }),
  );
});

afterEach(() => vi.unstubAllGlobals());

function Host({ start }: { start: PipelineForm }): JSX.Element {
  const [draft, setDraft] = useState<PipelineForm>(start);
  return (
    <WorkbenchSchedule
      project="city"
      draft={draft}
      onChange={(next) => {
        seen.push(next);
        setDraft(next);
      }}
    />
  );
}

const show = (start: PipelineForm) =>
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <Host start={start} />
      </I18nextProvider>
    </QueryClientProvider>,
  );

describe("the workbench's When it runs step", () => {
  it("sets a daily 03:00 CKAN load with no cron typed, and reads a typed cron back as its choice", async () => {
    const user = userEvent.setup();
    const { container } = show({ class: "auto", source: { dataSourceRef: "ckan-trees" } });
    await user.click(screen.getByRole("button", { name: w.add }));
    expect(seen.at(-1)).toMatchObject({ schedule: "0 3 * * *", class: "scheduled" });
    const how = screen.getByRole("combobox", { name: en.pipelines.schedule.how });
    expect(how).toHaveValue("daily");
    expect(screen.getByLabelText(en.pipelines.schedule.atTime)).toHaveValue("03:00");
    expect(within(screen.getByRole("list", { name: en.pipelines.schedule.next })).getAllByRole("listitem")).toHaveLength(5);
    // choice → cron
    await user.selectOptions(how, "weekly");
    expect(seen.at(-1)?.schedule).toBe("0 3 * * 1");
    // cron → choice
    const cron = screen.getByRole("textbox", { name: new RegExp(en.pipelines.schedule.cron.replace(/[()]/g, ".")) });
    await user.clear(cron);
    await user.type(cron, "30 4 * * *");
    expect(how).toHaveValue("daily");
    expect(screen.getByLabelText(en.pipelines.schedule.atTime)).toHaveValue("04:30");
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);
    await user.click(screen.getByRole("button", { name: w.remove }));
    expect(seen.at(-1)).toMatchObject({ schedule: undefined, class: "auto" });
  });

  it("refuses a schedule faster than the project's minimum at the field, naming the floor", async () => {
    floor = 3600;
    const user = userEvent.setup();
    show({ class: "scheduled", schedule: FIRST_SCHEDULE });
    expect(await screen.findByText(/at most once an hour/)).toBeInTheDocument();
    await user.selectOptions(screen.getByRole("combobox", { name: en.pipelines.schedule.how }), "minutes");
    expect(await screen.findByText(/runs every 15 minutes, more often than this project allows \(once an hour\)/)).toBeInTheDocument();
  });

  it("says a resident pipeline runs as records arrive, and a push source has no clock", () => {
    show({ class: "resident" });
    expect(screen.getByText(w.resident)).toBeInTheDocument();
  });

  it("keeps an explicit class and measures the shortest gap of a schedule", () => {
    expect(scheduled({ class: "resident" }, "0 3 * * *").class).toBe("resident");
    expect(scheduled(undefined, "0 3 * * *").class).toBe("scheduled");
    expect(scheduleGap("*/15 * * * *")).toBe(900);
    expect(scheduleGap("not a cron")).toBeUndefined();
    const say = (key: string, options: Record<string, unknown>) => i18n.t(key, options);
    expect([clockWords(3600, say), clockWords(7200, say), clockWords(900, say), clockWords(90, say)]).toEqual(["once an hour", "every 2 hours", "every 15 minutes", "every 90 seconds"]);
  });
});
