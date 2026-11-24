/**
 * EntityFilters branches the page tests leave open: the area (scopeQ) input, a boolean, a choice and
 * a text value, removing a row, and the attributes of a space whose model declares no type.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { EntityFilters } from "../src/components/entities/EntityFilters";
import type { EntityQuery, FilterSlot } from "../src/components/entities/filters";

const E = en.entities;

const SLOTS: FilterSlot[] = [
  { name: "active", range: "boolean", kind: "Property" },
  { name: "level", range: "AirLevel", kind: "Property", values: ["good", "bad"] },
  { name: "station", range: "string", kind: "Property" },
];

function Harness({ start, seen, slots = SLOTS, types = ["Station"], space }: { start: EntityQuery; seen: (next: EntityQuery) => void; slots?: FilterSlot[]; types?: string[]; space?: string }) {
  const [value, setValue] = useState<EntityQuery>(start);
  return (
    <EntityFilters
      id="x"
      types={types}
      slots={slots}
      space={space}
      value={value}
      onChange={(next) => {
        setValue(next);
        seen(next);
      }}
    />
  );
}

function mount(node: React.ReactNode) {
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
}

describe("EntityFilters inputs", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("sets and clears the area, and states a boolean, a choice and a text as the q", async () => {
    const user = userEvent.setup();
    const seen = vi.fn();
    mount(<Harness start={{ type: "Station" }} seen={seen} />);
    await user.type(screen.getByLabelText(E.scopeQ), "/city");
    expect(seen).toHaveBeenLastCalledWith({ type: "Station", scopeQ: "/city" });
    await user.clear(screen.getByLabelText(E.scopeQ));
    expect(seen).toHaveBeenLastCalledWith({ type: "Station", scopeQ: undefined });

    await user.click(screen.getByRole("button", { name: E.addFilter }));
    await user.selectOptions(screen.getByRole("combobox", { name: E.attribute }), "active");
    await user.selectOptions(screen.getByRole("combobox", { name: E.value }), "true");
    expect(seen).toHaveBeenLastCalledWith(expect.objectContaining({ q: "active==true" }));

    await user.selectOptions(screen.getByRole("combobox", { name: E.attribute }), "level");
    await user.selectOptions(screen.getByRole("combobox", { name: E.value }), "bad");
    expect(seen).toHaveBeenLastCalledWith(expect.objectContaining({ q: 'level=="bad"' }));

    await user.selectOptions(screen.getByRole("combobox", { name: E.attribute }), "station");
    await user.type(screen.getByRole("textbox", { name: E.value }), "Radvan");
    expect(seen).toHaveBeenLastCalledWith(expect.objectContaining({ q: 'station=="Radvan"' }));
  });

  it("removes a filter row and the q with it", async () => {
    const user = userEvent.setup();
    const seen = vi.fn();
    mount(<Harness start={{ type: "Station", q: "active==true" }} seen={seen} />);
    expect(screen.getByRole("combobox", { name: E.attribute })).toHaveValue("active");
    await user.click(screen.getByRole("button", { name: E.remove }));
    expect(seen).toHaveBeenLastCalledWith({ type: "Station", q: undefined });
    expect(screen.queryByRole("combobox", { name: E.attribute })).toBeNull();
  });
});

describe("EntityFilters of a space with no declared type", () => {
  let originalFetch: typeof global.fetch;
  beforeEach(async () => {
    originalFetch = global.fetch;
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    global.fetch = originalFetch;
  });

  it("offers the attributes the broker lists for the chosen type", async () => {
    const asked: string[] = [];
    global.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      asked.push(new URL(request.url).pathname + new URL(request.url).search);
      return new Response(JSON.stringify([{ typeName: "Bike", attributeNames: ["seats", "colour"] }]), { status: 200, headers: { "Content-Type": "application/json" } });
    }) as typeof fetch;
    mount(<Harness start={{ type: "Bike", q: "seats==4" }} seen={() => {}} slots={[]} types={[]} space="mobility" />);
    const attribute = await screen.findByRole("combobox", { name: E.attribute });
    await screen.findByRole("option", { name: "colour" });
    expect(attribute).toHaveValue("seats");
    expect(asked).toEqual(["/cs/mobility/ngsi-ld/v1/types?details=true"]);
  });
});
