/**
 * T-7585: the outputs field adds a second output next to one it already holds, from the same
 * class, under a free name; and a model list that failed to load says so and asks again.
 */
import { useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { FieldProps } from "@rjsf/utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { queryKeys } from "../src/api/client";
import { FormDataContext } from "../src/components/forms/widgets/EntitySelectorField";
import { FormProjectContext } from "../src/components/forms/widgets/ModelWidgets";
import { RepresentationOutputsField } from "../src/components/forms/widgets/RepresentationOutputsField";

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

function show(initial: unknown, seeded: boolean) {
  const seen: unknown[] = [];
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  if (seeded) {
    client.setQueryData(queryKeys.list("city", "datamodels"), { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  }
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <FormProjectContext.Provider value="city">
          <FormDataContext.Provider value={ON}>
            <Held initial={initial} seen={seen} />
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

describe("a second output", () => {
  it("takes the class of the first and a name that is free", async () => {
    const seen = show({ ical: [{ name: "kalendar", sources: [{ type: "Sensor", preset: "sdm-event" }] }] }, true);
    await userEvent.click(screen.getByRole("button", { name: en.endpoints.outputs.ical.add }));
    expect(seen.at(-1)).toEqual({
      ical: [
        { name: "kalendar", sources: [{ type: "Sensor", preset: "sdm-event" }] },
        { name: "kalendar-2", sources: [{ type: "Sensor", preset: "sdm-event" }] },
      ],
    });
  });
});

describe("a model list that failed", () => {
  it("names the refusal and asks the server again on Retry", async () => {
    const fetched = vi.fn(
      async () =>
        new Response(JSON.stringify({ title: "Boom", status: 500, detail: "the models are not ready" }), {
          status: 500,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );
    vi.stubGlobal("fetch", fetched);
    show(undefined, false);
    const retry = await screen.findByRole("button", { name: en.form.listRetry });
    expect(screen.getByText(/the models are not ready/)).toBeInTheDocument();
    const before = fetched.mock.calls.length;
    await userEvent.click(retry);
    await waitFor(() => expect(fetched.mock.calls.length).toBeGreaterThan(before));
  });
});
