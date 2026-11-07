/**
 * The endpoint's class choice is part of what the form asks for (T-2258; UI-02, PF-57, EP-47).
 *
 * Measured on dev on 2026-09-19 by `ui/e2e/live/forms-examples.spec.ts`: every field the endpoint
 * form marks required was filled from its own example, Check was pressed, and the check never ran —
 * "Tick at least one class to expose through the endpoint", a sentence that appeared under the
 * button after the attempt. A choice the form refuses to go on without belongs beside the ticks,
 * with the same one action every other field offers.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ModelPicker } from "../src/pages/endpoints/ModelPicker";
import type { ModelPickerState } from "../src/pages/endpoints/ModelPicker";

const LINKML = `id: https://hel.fi/models/air
name: air
slots:
  id:
    range: uriorcurie
  type:
    range: string
  pm10:
    range: float
  pm25:
    range: float
  depotName:
    range: string
classes:
  AirQualityObserved:
    slots: [id, type, pm10, pm25]
  Depot:
    slots: [id, type, depotName]
`;

const MODELS = {
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataModel",
      metadata: { name: "air", namespace: "helsinki" },
      spec: { contextSpaceRef: "ovzdusie", version: "1", linkml: LINKML },
      status: { phase: "Live" },
    },
  ],
};

beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string) => {
      const url = typeof input === "string" ? input : input.url;
      const body = url.includes("/datamodels") ? MODELS : { items: [] };
      return new Response(JSON.stringify(body), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function renderPicker(onChange: (next: ModelPickerState) => void) {
  const value: ModelPickerState = { projectionName: "public-air", classes: {} };
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <ModelPicker
          project="helsinki"
          spaceName="ovzdusie"
          endpointName="public-air"
          value={value}
          onChange={onChange}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the class the endpoint publishes", () => {
  it("says what it still needs, beside the ticks and before any check", async () => {
    renderPicker(() => {});
    const note = await screen.findByRole("note");
    expect(note).toHaveTextContent(en.endpoints.picker.nothingTicked);
  });

  it("offers the same one action every other field offers, and it ticks one class", async () => {
    const user = userEvent.setup();
    const changes: ModelPickerState[] = [];
    renderPicker((next) => changes.push(next));

    const note = await screen.findByRole("note");
    await user.click(await within(note).findByRole("button", { name: en.form.useExample }));

    const ticked = changes.at(-1)?.classes ?? {};
    expect(Object.keys(ticked), "the first class of the model, and only it").toEqual([
      "AirQualityObserved",
    ]);
    expect(ticked.AirQualityObserved.ticked).toBe(true);
    // Its own attributes, never the identity slots the projection always carries.
    expect(ticked.AirQualityObserved.slots).toEqual(["pm10", "pm25"]);
  });
});
