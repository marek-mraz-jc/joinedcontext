// T-7706: the calendar output field drops a title that was emptied, and a preview shows a start it cannot read as it came.
import { useState } from "react";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { FieldProps } from "@rjsf/utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { queryKeys } from "../src/api/client";
import { FormDataContext } from "../src/components/forms/widgets/EntitySelectorField";
import { RepresentationOutputsField } from "../src/components/forms/widgets/RepresentationOutputsField";
import { FormProjectContext } from "../src/components/forms/widgets/ModelWidgets";

const o = en.endpoints.outputs;
const LINKML = "id: x\nname: x\nclasses:\n  Event:\n    slots: [name, startDate, title]\nslots:\n  name: { required: true }\n  startDate: { range: datetime, required: true }\n  title: {}\n";
const ON = { contextSpaceRef: "events", enabledRepresentations: ["ngsi-ld", "ical"] };

function Held({ initial, seen }: { initial?: unknown; seen: unknown[] }) {
  const [value, setValue] = useState<unknown>(initial);
  const props = {
    formData: value,
    onChange: (next: unknown) => {
      seen.push(next);
      setValue(next);
    },
    schema: { type: "object", title: "Calendars and news feeds" },
    fieldPathId: { $id: "root_representations", path: ["representations"] },
    registry: {},
  } as unknown as FieldProps;
  return <RepresentationOutputsField {...props} />;
}

function show(initial?: unknown, form: unknown = ON, linkml = LINKML) {
  const seen: unknown[] = [];
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(queryKeys.list("city", "datamodels"), {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "List",
    items: [
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "DataModel",
        metadata: { name: "events", namespace: "city" },
        spec: { contextSpaceRef: "events", linkml, version: "1.0.0" },
      },
    ],
  });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <FormProjectContext.Provider value="city">
          <FormDataContext.Provider value={form}>
            <div data-testid="form-page">
              <Held initial={initial} seen={seen} />
            </div>
          </FormDataContext.Provider>
        </FormProjectContext.Provider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return seen;
}


const kultura = { name: "kultura", query: "q=eventCategory==%22koncert%22", sources: [{ type: "Event", preset: "sdm-event" }] };
const SAVED = { ...ON, name: "events-bb", slug: "w3nq7pzk2mtv6hx5brj4gcdsfa" };

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the calendar outputs, the edges", () => {
  it("drops the title when its text is emptied", async () => {
    const seen = show();
    await userEvent.click(screen.getByRole("button", { name: o.ical.add }));
    const title = screen.getByLabelText(o.title.sk);
    await userEvent.type(title, "Šport");
    expect(seen.at(-1)).toMatchObject({ ical: [{ title: { sk: "Šport" } }] });
    await userEvent.clear(title);
    expect(seen.at(-1)).toEqual({ ical: [{ name: "kalendar", sources: [{ type: "Event", preset: "sdm-event" }] }] });
  });

  it("lists a preview entry whose start is no date as it came", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(
          JSON.stringify({ read: 1, valid: 1, outside: 0, skipped: {}, events: ["BEGIN:VEVENT\r\nDTSTART:soon\r\nSUMMARY:Koncert\r\nEND:VEVENT\r\n"] }),
          { headers: { "Content-Type": "application/json" } },
        ),
      ),
    );
    show({ ical: [kultura] }, SAVED);
    await userEvent.click(screen.getByRole("button", { name: o.preview.label }));
    expect(await screen.findByText("Koncert")).toBeInTheDocument();
    expect(screen.getByText("soon")).toBeInTheDocument();
  });
});
