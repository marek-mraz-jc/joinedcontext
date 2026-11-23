/**
 * T-7585: the schedule widget's remaining choices (every N hours, hourly, daily, custom), its
 * minute and time fields, and the source's frequency read from the server.
 */
import { useState } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { WidgetProps } from "@rjsf/utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { CronScheduleWidget } from "../src/components/forms/widgets/CronSchedule";
import { FormDataContext } from "../src/components/forms/widgets/EntitySelectorField";
import { FormProjectContext } from "../src/components/forms/widgets/ModelWidgets";

const s = en.pipelines.schedule;

function Held({ initial, seen, blurred }: { initial?: string; seen: (value: unknown) => void; blurred?: (id: string, value: unknown) => void }) {
  const [value, setValue] = useState<unknown>(initial);
  const props = {
    id: "root_schedule",
    name: "schedule",
    value,
    onChange: (next: unknown) => {
      seen(next);
      setValue(next);
    },
    onBlur: blurred ?? (() => undefined),
    onFocus: () => undefined,
    options: {},
    schema: { type: "string" },
    label: "Schedule",
    registry: {},
  } as unknown as WidgetProps;
  return <CronScheduleWidget {...props} />;
}

function show(initial?: string, form: unknown = {}, blurred?: (id: string, value: unknown) => void) {
  const seen: unknown[] = [];
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <FormProjectContext.Provider value="helsinki">
          <FormDataContext.Provider value={form}>
            <Held initial={initial} seen={(value) => seen.push(value)} blurred={blurred} />
          </FormDataContext.Provider>
        </FormProjectContext.Provider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return seen;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the choices of a schedule", () => {
  it("writes every N hours at a minute, hourly at a minute and daily at a time", async () => {
    const user = userEvent.setup();
    const seen = show("0 3 * * *");
    await user.selectOptions(screen.getByLabelText(s.how), "hours");
    expect(seen.at(-1)).toBe("0 */6 * * *");
    await user.selectOptions(screen.getByLabelText(s.how), "hourly");
    expect(seen.at(-1)).toBe("0 * * * *");
    const minute = screen.getByLabelText(s.atMinute);
    fireEvent.change(minute, { target: { value: "30" } });
    expect(seen.at(-1)).toBe("30 * * * *");
    fireEvent.change(minute, { target: { value: "75" } });
    expect(seen.at(-1)).toBe("30 * * * *");
    await user.selectOptions(screen.getByLabelText(s.how), "daily");
    expect(seen.at(-1)).toBe("30 3 * * *");
    const time = screen.getByLabelText(s.atTime);
    fireEvent.change(time, { target: { value: "07:45" } });
    expect(seen.at(-1)).toBe("45 7 * * *");
    fireEvent.change(time, { target: { value: "" } });
    expect(seen.at(-1)).toBe("45 7 * * *");
  });

  it("starts a custom schedule from the daily one when nothing is written yet", async () => {
    const seen = show();
    await userEvent.selectOptions(screen.getByLabelText(s.how), "custom");
    expect(seen.at(-1)).toBe("0 3 * * *");
    expect(screen.getByLabelText(s.cron)).toHaveValue("0 3 * * *");
  });
});

describe("the cron text", () => {
  it("reports its text when the field is left", async () => {
    const blurred = vi.fn();
    show("0 3 * * *", {}, blurred);
    await userEvent.click(screen.getByLabelText(s.cron));
    await userEvent.tab();
    expect(blurred).toHaveBeenCalledWith("root_schedule", "0 3 * * *");
  });
});

describe("the frequency of the source a pipeline reads", () => {
  it("is asked of the server and warns when the schedule is more frequent", async () => {
    const asked = vi.fn(async () =>
      Response.json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "List",
        items: [
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Endpoint",
            metadata: { name: "air-open", namespace: "helsinki" },
            spec: { contextSpaceRef: "air", catalog: { frequency: "DAILY" } },
          },
        ],
      }),
    );
    vi.stubGlobal("fetch", asked);
    show("*/15 * * * *", { source: { endpointRef: "air-open" } });
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("its catalogue record says: Every day"));
    expect(asked).toHaveBeenCalled();
    expect(((asked.mock.calls[0] as unknown[])[0] as Request).url).toContain("/api/v1/projects/helsinki/endpoints");
  });
});
