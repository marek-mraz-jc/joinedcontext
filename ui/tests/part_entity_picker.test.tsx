/**
 * T-1818: the widget that picks an entity out of a space, against the UI contract
 * (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * It is a combobox over live gateway data, so the contract is mostly about the keyboard and about
 * what it says when it cannot do its job: both controls are the shared `Input`, the list is walked
 * and chosen with the arrow keys and Enter, and a widget without a space and a type says the field
 * is misconfigured instead of offering an empty box that would never answer.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WidgetProps } from "@rjsf/utils";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { EntityPicker } from "../src/components/forms/widgets/EntityPicker";
import { expectNoRawKeys, expectNoViolations } from "./checks";

const ENTITIES = [
  { id: "urn:ngsi-ld:Sensor:radvan-01", type: "Sensor", name: { type: "Property", value: "Radvaň 01" } },
  { id: "urn:ngsi-ld:Sensor:sasova-02", type: "Sensor", name: { type: "Property", value: "Sásová 02" } },
];

function props(overrides?: Partial<WidgetProps>): WidgetProps {
  return {
    id: "root_sensor",
    name: "sensor",
    schema: {},
    value: undefined,
    required: false,
    disabled: false,
    readonly: false,
    autofocus: false,
    options: { space: "ovzdusie", entityType: "Sensor" },
    label: "Sensor",
    onChange: vi.fn(),
    onBlur: vi.fn(),
    onFocus: vi.fn(),
    registry: {} as WidgetProps["registry"],
    ...overrides,
  };
}

function show(overrides?: Partial<WidgetProps>) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const given = props(overrides);
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        {/* The field above names the control; here the label is the test's own. */}
        <label htmlFor="root_sensor">{given.label}</label>
        <EntityPicker {...given} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { ...view, given, user: userEvent.setup() };
}

function stub(answer: unknown[] | number = ENTITIES) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(
        typeof answer === "number"
          ? new Response(JSON.stringify({ title: "Server Error" }), { status: answer })
          : new Response(JSON.stringify(answer), {
              status: 200,
              headers: { "Content-Type": "application/ld+json" },
            }),
      ),
    ),
  );
}

const box = () => screen.getByRole("combobox", { name: "Sensor" }) as HTMLInputElement;

describe("the entity picker against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("is the shared control, and says what it is to a screen reader", () => {
    stub();
    const { container } = show();

    expect(box().tagName).toBe("INPUT");
    expect(box().className).toContain("focus-ring");
    expect(box()).toHaveAttribute("aria-expanded", "false");
    expect(box()).toHaveAttribute("aria-autocomplete", "list");
    expect(container.querySelector("#root_sensor__listbox")).toBeNull();
  });

  it("is chosen by the arrow keys and Enter, and hands back the entity's own id", async () => {
    stub();
    const { user, given } = show();

    await user.type(box(), "rad");
    const options = await screen.findAllByRole("option");
    expect(options[0]).toHaveTextContent("Radvaň 01");

    await user.keyboard("{ArrowDown}{Enter}");
    expect(given.onChange).toHaveBeenCalledWith(ENTITIES[0].id);
  });

  it("has no axe violation, closed and with the list open", async () => {
    stub();
    const { container, user } = show();

    await expectNoViolations(container);
    await user.type(box(), "rad");
    await screen.findAllByRole("option");
    await expectNoViolations(container);
  });

  it("says a field with no space and no type is misconfigured, and offers nothing", async () => {
    stub();
    const { container } = show({ options: {} });

    // UI-44: refused and explained, rather than an empty box that will never answer.
    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("form.invalid"));
    const input = container.querySelector("input") as HTMLInputElement;
    expect(input).toBeDisabled();
    expect(input).toHaveAttribute("aria-disabled", "true");
    await expectNoViolations(container);
  });

  it("says the read failed rather than pretending the space is empty", async () => {
    stub(500);
    const { user } = show();

    await user.type(box(), "rad");
    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("app.error.generic"));
  });

  it.each(SUPPORTED_LOCALES)("writes its own words in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    stub([]);
    const { container, user } = show();

    await user.type(box(), "rad");
    await waitFor(() => expect(screen.getByRole("listbox")).toBeInTheDocument());
    expectNoRawKeys(container);
  });
});
