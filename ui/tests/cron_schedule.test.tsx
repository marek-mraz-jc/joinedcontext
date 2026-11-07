/**
 * T-3259: a pipeline's schedule picked in words writes the cron, a cron typed by hand shows as the
 * choice it says, the next five runs show in the person's time, and a schedule more frequent than
 * the source's catalogue record says it changes is warned about.
 */
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { WidgetProps } from "@rjsf/utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { queryKeys } from "../src/api/client";
import { CronScheduleWidget } from "../src/components/forms/widgets/CronSchedule";
import { FormDataContext } from "../src/components/forms/widgets/EntitySelectorField";
import { FormProjectContext } from "../src/components/forms/widgets/ModelWidgets";

const s = en.pipelines.schedule;

function Held({ initial, seen }: { initial?: string; seen: (value: unknown) => void }) {
  const [value, setValue] = useState<unknown>(initial);
  const props = {
    id: "root_schedule",
    name: "schedule",
    value,
    onChange: (next: unknown) => {
      seen(next);
      setValue(next);
    },
    onBlur: () => undefined,
    onFocus: () => undefined,
    options: {},
    schema: { type: "string" },
    label: "Schedule",
    registry: {},
  } as unknown as WidgetProps;
  return <CronScheduleWidget {...props} />;
}

function show(initial?: string, form: unknown = {}, frequency?: string) {
  const seen: unknown[] = [];
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(queryKeys.list("helsinki", "endpoints"), {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "List",
    items: [
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Endpoint",
        metadata: { name: "air-open", namespace: "helsinki" },
        spec: { contextSpaceRef: "air", ...(frequency ? { catalog: { frequency } } : {}) },
      },
    ],
  });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <FormProjectContext.Provider value="helsinki">
          <FormDataContext.Provider value={form}>
            <Held initial={initial} seen={(value) => seen.push(value)} />
          </FormDataContext.Provider>
        </FormProjectContext.Provider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return seen;
}

describe("a schedule in words (PL-69)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date("2026-10-07T22:47:30Z"));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("writes the cron of what a person picks", async () => {
    const user = userEvent.setup();
    const seen = show();
    await user.selectOptions(screen.getByLabelText(s.how), "minutes");
    expect(seen.at(-1)).toBe("*/15 * * * *");
    await user.selectOptions(screen.getByLabelText(s.every.minutes), "5");
    expect(seen.at(-1)).toBe("*/5 * * * *");
    await user.selectOptions(screen.getByLabelText(s.how), "weekly");
    await user.selectOptions(screen.getByLabelText(s.on), "Monday");
    expect(seen.at(-1)).toBe("0 3 * * 1");
    expect(screen.getByLabelText(s.cron)).toHaveValue("0 3 * * 1");
  });

  it("shows a cron typed by hand as the choice it says, and the next five runs", async () => {
    show("0 3 * * *");
    expect(screen.getByLabelText(s.how)).toHaveValue("daily");
    expect(screen.getByLabelText(s.atTime)).toHaveValue("03:00");
    const runs = screen.getByRole("list", { name: s.next }).querySelectorAll("time");
    expect([...runs].map((time) => time.getAttribute("datetime"))).toEqual([
      "2026-10-08T03:00:00.000Z",
      "2026-10-09T03:00:00.000Z",
      "2026-10-10T03:00:00.000Z",
      "2026-10-11T03:00:00.000Z",
      "2026-10-12T03:00:00.000Z",
    ]);
  });

  it("keeps a custom cron as written and says when it is not one", async () => {
    const user = userEvent.setup();
    const seen = show("0 8-10/2 * * *");
    expect(screen.getByLabelText(s.how)).toHaveValue("custom");
    const cron = screen.getByLabelText(s.cron);
    await user.clear(cron);
    await user.type(cron, "0 99 * * *");
    expect(seen.at(-1)).toBe("0 99 * * *");
    expect(screen.getByRole("alert")).toHaveTextContent(s.invalid);
    expect(cron).toHaveAttribute("aria-invalid", "true");
    expect(screen.queryByRole("list", { name: s.next })).toBeNull();
  });

  it("warns when the schedule asks the source more often than its catalogue says it changes", () => {
    show("*/15 * * * *", { source: { endpointRef: "air-open" } }, "DAILY");
    expect(screen.getByRole("status")).toHaveTextContent("its catalogue record says: Every day");
  });

  it("says nothing of a source whose frequency is not known, or one slower than the schedule", () => {
    show("*/15 * * * *", { source: { endpointRef: "air-open" } });
    expect(screen.queryByText(/catalogue record says/)).toBeNull();
  });
});
