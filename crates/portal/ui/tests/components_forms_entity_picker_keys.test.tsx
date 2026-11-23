/**
 * T-7585: the entity picker by keyboard and focus: arrows open and wrap, Escape closes, an id
 * typed whole is taken on Enter, a click outside closes the list, a read-only picker ignores keys,
 * and the typed-in fallback while the form's space is not chosen reports blur.
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { WidgetProps } from "@rjsf/utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { EntityPicker } from "../src/components/forms/widgets/EntityPicker";

function propsOf(overrides: Partial<WidgetProps> = {}): WidgetProps {
  return {
    id: "picker",
    name: "picker",
    schema: {},
    value: undefined,
    required: false,
    disabled: false,
    readonly: false,
    autofocus: false,
    options: { space: "test-space", entityType: "Sensor" },
    label: "Sensor Picker",
    onChange: vi.fn(),
    onBlur: vi.fn(),
    onFocus: vi.fn(),
    registry: {} as WidgetProps["registry"],
    ...overrides,
  };
}

const ENTITIES = [
  { id: "urn:ngsi-ld:Sensor:001", type: "Sensor", name: { type: "Property", value: "North" } },
  { id: "urn:ngsi-ld:Sensor:002", type: "Sensor", name: { type: "Property", value: "South" } },
];

function show(props: WidgetProps) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(JSON.stringify(ENTITIES), { status: 200, headers: { "Content-Type": "application/ld+json" } })),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <div data-testid="outside">elsewhere</div>
        <EntityPicker {...props} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("the entity picker by keyboard", () => {
  it("moves up from nothing to the last entry, wraps down to the first, and Escape closes the list", async () => {
    show(propsOf());
    const box = screen.getByRole("combobox");
    await userEvent.click(box);
    await screen.findByRole("option", { name: "South" });
    await userEvent.keyboard("{ArrowUp}");
    expect(box).toHaveAttribute("aria-activedescendant", "picker__option-1");
    await userEvent.keyboard("{ArrowDown}");
    expect(box).toHaveAttribute("aria-activedescendant", "picker__option-0");
    await userEvent.keyboard("{Escape}");
    expect(box).toHaveAttribute("aria-expanded", "false");
    expect(box).not.toHaveAttribute("aria-activedescendant");
  });

  it("opens the closed list with an arrow key, up or down", async () => {
    show(propsOf());
    const box = screen.getByRole("combobox");
    await userEvent.click(box);
    await userEvent.keyboard("{Escape}");
    await userEvent.keyboard("{ArrowDown}");
    expect(box).toHaveAttribute("aria-expanded", "true");
    await userEvent.keyboard("{Escape}");
    await userEvent.keyboard("{ArrowUp}");
    expect(box).toHaveAttribute("aria-expanded", "true");
  });

  it("takes an id typed whole on Enter, without choosing from the list", async () => {
    const props = propsOf();
    show(props);
    await userEvent.type(screen.getByRole("combobox"), "urn:ngsi-ld:Sensor:009{Enter}");
    expect(props.onChange).toHaveBeenLastCalledWith("urn:ngsi-ld:Sensor:009");
  });

  it("closes the list on a click outside it", async () => {
    show(propsOf());
    const box = screen.getByRole("combobox");
    await userEvent.click(box);
    expect(box).toHaveAttribute("aria-expanded", "true");
    fireEvent.mouseDown(screen.getByTestId("outside"));
    expect(box).toHaveAttribute("aria-expanded", "false");
  });

  it("ignores keys on a read-only picker", async () => {
    show(propsOf({ readonly: true }));
    const box = screen.getByRole("combobox");
    fireEvent.keyDown(box, { key: "ArrowDown" });
    await waitFor(() => expect(box).toHaveAttribute("aria-expanded", "false"));
  });
});

describe("the entity picker before its space is chosen", () => {
  it("lets the id be typed and reports focus and blur", async () => {
    const props = propsOf({ options: { dependent: true } });
    show(props);
    const box = screen.getByRole("textbox");
    await userEvent.type(box, "x");
    expect(props.onChange).toHaveBeenLastCalledWith("x");
    expect(props.onFocus).toHaveBeenCalled();
    await userEvent.tab();
    expect(props.onBlur).toHaveBeenCalledWith("picker", undefined);
  });
});
