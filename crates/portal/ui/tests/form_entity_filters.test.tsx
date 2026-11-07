/**
 * T-1756: the EntityFilters form against the UI contract (UI-04, UI-15, UI-16, UI-33, UI-44).
 *
 * No test in `ui/tests` or `ui/e2e` named this file before, and the attribute a caller may not
 * read was hard-`disabled` in a hand-written label: it left the tab order, so the reason survived
 * only in a `title` on an element a keyboard can never land on, and somebody working by keyboard
 * saw a struck-through name with no way to find out why (T-0529, `PermissionGuard.tsx:16-19`).
 * `checkForm` is T-1730's and joins this file with it.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { EntityFilters } from "../src/components/entities/EntityFilters";
import type { EntityQuery, FilterSlot } from "../src/components/entities/filters";
import {
  expectDenied,
  expectNoRawKeys,
  expectNoViolations,
  expectOpen,
  expectTabOrder,
  focusables,
} from "./checks";

const TYPES = ["AirQualityObserved", "Device"];
const SLOTS: FilterSlot[] = [
  { name: "pm10", range: "float", kind: "Property" },
  { name: "active", range: "boolean", kind: "Property" },
  { name: "owner", range: "string", kind: "Property" },
];

const DENIED = { owner: "Your role may not read owner in this space." };

function show(
  value: EntityQuery = { type: "AirQualityObserved" },
  denied: Record<string, string> = {},
) {
  const onChange = vi.fn();
  const rendered = render(
    <I18nextProvider i18n={i18n}>
      <EntityFilters
        id="q"
        types={TYPES}
        slots={SLOTS}
        value={value}
        denied={denied}
        onChange={onChange}
      />
    </I18nextProvider>,
  );
  return { ...rendered, onChange };
}

const attribute = (name: string) => screen.getByRole("checkbox", { name: new RegExp(`^${name}`) });

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("an attribute the caller may not read (UI-44, T-0529)", () => {
  it("is_reachable_by_keyboard_and_carries_the_reason_as_its_description", () => {
    const { container } = show({ type: "AirQualityObserved" }, DENIED);
    const box = attribute("owner");
    expectDenied(box, DENIED.owner);
    expect(focusables(container)).toContain(box);
  });

  it("stays_unticked_when_the_space_bar_is_pressed_on_it", async () => {
    const { onChange } = show({ type: "AirQualityObserved" }, DENIED);
    const box = attribute("owner");
    box.focus();
    await userEvent.keyboard(" ");
    expect(box).not.toBeChecked();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("stays_unticked_when_it_is_clicked", async () => {
    const { onChange } = show({ type: "AirQualityObserved" }, DENIED);
    await userEvent.click(attribute("owner"));
    expect(attribute("owner")).not.toBeChecked();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("an_attribute_nobody_refused_is_open_and_ticking_it_asks_for_it", async () => {
    const { onChange } = show({ type: "AirQualityObserved" }, DENIED);
    expectOpen(attribute("pm10"));
    await userEvent.click(attribute("pm10"));
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ attrs: ["pm10"] }),
    );
  });

  it("unticking_the_last_attribute_asks_for_all_of_them_again", async () => {
    const { onChange } = show({ type: "AirQualityObserved", attrs: ["pm10"] }, DENIED);
    expect(attribute("pm10")).toBeChecked();
    await userEvent.click(attribute("pm10"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ attrs: undefined }));
  });
});

describe("the filters the model generates (UI-33)", () => {
  it("the_type_is_chosen_from_what_the_model_declares", async () => {
    const { onChange } = show({});
    await userEvent.selectOptions(screen.getByLabelText(en.entities.type), "Device");
    expect(onChange).toHaveBeenCalledWith({ type: "Device", scopeQ: undefined });
  });

  it("a_model_that_declares_no_type_and_no_space_says_so_instead_of_a_text_box", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <EntityFilters id="q" types={[]} slots={[]} value={{}} onChange={() => {}} />
      </I18nextProvider>,
    );
    expect(screen.getByLabelText(en.entities.type)).toBeDisabled();
    expect(screen.getByLabelText(en.entities.type)).toHaveAccessibleDescription(en.entities.noTypes);
  });

  it("a_model_that_declares_no_type_offers_the_types_the_space_lets_this_person_read", async () => {
    const fetched: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        fetched.push(String(input));
        return new Response(JSON.stringify([{ id: "urn:x", type: "EntityType", typeName: "Device", attributeNames: ["battery"] }]), {
          status: 200,
          headers: { "Content-Type": "application/ld+json" },
        });
      }),
    );
    const onChange = vi.fn();
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <EntityFilters id="q" types={[]} space="air" slots={[]} value={{ type: "Device", q: "battery==1" }} onChange={onChange} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(screen.getByLabelText(en.entities.type)).toBeEnabled());
    expect(within(screen.getByLabelText(en.entities.type)).getByRole("option", { name: "Device" })).toBeInTheDocument();
    expect(within(screen.getByLabelText(en.entities.attribute)).getByRole("option", { name: "battery" })).toBeInTheDocument();
    expect(fetched).toEqual(["/cs/air/ngsi-ld/v1/types?details=true"]);
    vi.unstubAllGlobals();
  });

  it("a_row_composes_the_q_the_query_sends", async () => {
    const { onChange } = show({ type: "AirQualityObserved" });
    await userEvent.click(screen.getByRole("button", { name: en.entities.addFilter }));
    expect(onChange).toHaveBeenCalled();
  });

  it("a_q_the_rows_cannot_show_is_said_in_words_and_kept_as_text", () => {
    show({ type: "AirQualityObserved", q: "pm10>10|active==true" });
    expect(screen.getByText(en.entities.advanced)).toBeInTheDocument();
    expect(screen.getByLabelText(en.entities.q)).toHaveValue("pm10>10|active==true");
  });

  it("the_q_field_says_what_it_is_for_before_the_box", () => {
    show({ type: "AirQualityObserved" });
    expect(screen.getByLabelText(en.entities.q)).toHaveAccessibleDescription(en.entities.hint.q);
  });
});

describe("the form meets the UI contract", () => {
  it("has_no_axe_violation_with_an_attribute_refused", async () => {
    const { container } = show({ type: "AirQualityObserved", q: "pm10>10" }, DENIED);
    await expectNoViolations(container);
  });

  it("every_control_is_reached_by_keyboard_in_dom_order", async () => {
    const user = userEvent.setup();
    const { container } = show({ type: "AirQualityObserved", q: "pm10>10" }, DENIED);
    await expectTabOrder(user, container);
  });

  it("the_attributes_are_grouped_under_a_legend_that_names_them", () => {
    show({ type: "AirQualityObserved" }, DENIED);
    // Inside the Columns menu (T-6092), the legend still names the group the boxes are in.
    const group = screen.getByText(en.entities.attrs).closest("fieldset")!;
    expect(within(group as HTMLElement).getAllByRole("checkbox").length).toBeGreaterThan(0);
    expect(screen.getByText(en.entities.columns).closest("details")).toContainElement(group as HTMLElement);
  });

  it.each(SUPPORTED_LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = show({ type: "AirQualityObserved" }, DENIED);
    // The type names and the attribute names come from the space's DataModel, not from a
    // bundle: `AirQualityObserved` and `pm10` are data and are shown as written.
    expectNoRawKeys(container, ["option", "fieldset:last-of-type"]);
  });
});
