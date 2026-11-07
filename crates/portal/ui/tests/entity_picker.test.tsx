import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WidgetProps } from "@rjsf/utils";
import i18n from "../src/i18n";
import { EntityPicker } from "../src/components/forms/widgets/EntityPicker";

function createMockProps(overrides?: Partial<WidgetProps>): WidgetProps {
  return {
    id: "picker",
    name: "picker",
    schema: {},
    value: undefined,
    required: false,
    disabled: false,
    readonly: false,
    autofocus: false,
    options: {
      space: "test-space",
      entityType: "Sensor",
    },
    label: "Sensor Picker",
    onChange: vi.fn(),
    onBlur: vi.fn(),
    onFocus: vi.fn(),
    registry: {} as WidgetProps["registry"],
    ...overrides,
  };
}

describe("EntityPicker", () => {
  let queryClient: QueryClient;
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    queryClient = new QueryClient({
      defaultOptions: {
        queries: {
          retry: false,
        },
      },
    });
    fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("searches entities on typing, displays name, and selects URN on click", async () => {
    const onChange = vi.fn();
    const props = createMockProps({ onChange });

    const mockEntities = [
      {
        id: "urn:ngsi-ld:Sensor:001",
        type: "Sensor",
        name: { type: "Property", value: "Temperature Sensor" },
      },
    ];

    fetchMock.mockImplementation(() =>
      Promise.resolve(
        new Response(JSON.stringify(mockEntities), {
          status: 200,
          headers: { "Content-Type": "application/ld+json" },
        })
      )
    );

    const user = userEvent.setup();

    render(
      <QueryClientProvider client={queryClient}>
        <I18nextProvider i18n={i18n}>
          <EntityPicker {...props} />
        </I18nextProvider>
      </QueryClientProvider>
    );

    const input = screen.getByRole("combobox");
    // F-R2UI2-023: aria-controls names the list only while it is in the page.
    expect(input).not.toHaveAttribute("aria-controls");
    await user.type(input, "foo");

    // The picker fires once with an empty term on mount, then again after the 250 ms debounce;
    // wait for the call that carries the typed filter.
    await waitFor(() => {
      const lastCall = fetchMock.mock.calls[fetchMock.mock.calls.length - 1];
      const requestedUrl = decodeURIComponent(String(lastCall?.[0]));
      expect(requestedUrl).toContain("type=Sensor");
      expect(requestedUrl).toContain('q=name~="foo"');
    });

    const option = await screen.findByRole("option", { name: "Temperature Sensor" });
    expect(option).toBeInTheDocument();
    expect(document.getElementById(input.getAttribute("aria-controls") ?? "")).toHaveAttribute("role", "listbox");
    expect(screen.queryByText("urn:ngsi-ld:Sensor:001")).not.toBeInTheDocument();

    await user.click(option);

    expect(onChange).toHaveBeenCalledWith("urn:ngsi-ld:Sensor:001");
    expect(input).toHaveAttribute("aria-expanded", "false");
  });

  it("selects active entity on Enter key and updates aria-expanded to false", async () => {
    const onChange = vi.fn();
    const props = createMockProps({ onChange });

    const mockEntities = [
      {
        id: "urn:ngsi-ld:Sensor:002",
        type: "Sensor",
        name: { type: "Property", value: "Pressure Sensor" },
      },
    ];

    fetchMock.mockImplementation(() =>
      Promise.resolve(
        new Response(JSON.stringify(mockEntities), {
          status: 200,
          headers: { "Content-Type": "application/ld+json" },
        })
      )
    );

    const user = userEvent.setup();

    render(
      <QueryClientProvider client={queryClient}>
        <I18nextProvider i18n={i18n}>
          <EntityPicker {...props} />
        </I18nextProvider>
      </QueryClientProvider>
    );

    const input = screen.getByRole("combobox");
    await user.type(input, "bar");


    const option = await screen.findByRole("option", { name: "Pressure Sensor" });
    expect(option).toBeInTheDocument();

    await user.keyboard("{Enter}");

    expect(onChange).toHaveBeenCalledWith("urn:ngsi-ld:Sensor:002");
    expect(input).toHaveAttribute("aria-expanded", "false");
  });

  it("calls onChange with undefined when the input is cleared", async () => {
    const onChange = vi.fn();
    const props = createMockProps({ value: "urn:ngsi-ld:Sensor:001", onChange });

    const user = userEvent.setup();

    render(
      <QueryClientProvider client={queryClient}>
        <I18nextProvider i18n={i18n}>
          <EntityPicker {...props} />
        </I18nextProvider>
      </QueryClientProvider>
    );

    const input = screen.getByRole("combobox");
    await user.clear(input);

    expect(onChange).toHaveBeenCalledWith(undefined);
  });

  it("renders disabled input and invalid hint when space or entityType is missing", () => {
    const props = createMockProps({
      options: {},
    });

    render(
      <QueryClientProvider client={queryClient}>
        <I18nextProvider i18n={i18n}>
          <EntityPicker {...props} />
        </I18nextProvider>
      </QueryClientProvider>
    );

    const input = screen.getByRole("textbox");
    expect(input).toBeDisabled();
    expect(screen.getByText("Invalid value")).toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
