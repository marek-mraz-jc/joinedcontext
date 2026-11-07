/**
 * UI-44: the picker of a project's own manifests says which of the two happened.
 *
 * A list that could not be asked for read exactly like an empty project — "No results" — in every
 * form that picks a resource, so a 403 on a kind this person may not read and a 500 from the Portal
 * both looked like a project with nothing in it (T-1486).
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WidgetProps } from "@rjsf/utils";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourcePicker } from "../src/components/forms/widgets/ResourcePicker";

function props(overrides?: Partial<WidgetProps>): WidgetProps {
  return {
    id: "root_dataSourceRef",
    name: "dataSourceRef",
    schema: {},
    value: undefined,
    required: false,
    disabled: false,
    readonly: false,
    autofocus: false,
    options: { plural: "datasources", project: "helsinki" },
    label: "Data source",
    onChange: vi.fn(),
    onBlur: vi.fn(),
    onFocus: vi.fn(),
    registry: {} as WidgetProps["registry"],
    ...overrides,
  };
}

function show(overrides?: Partial<WidgetProps>) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ResourcePicker {...props(overrides)} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const refusal = (detail: string, status = 403) =>
  new Response(JSON.stringify({ title: "Forbidden", status, detail }), {
    status,
    headers: { "content-type": "application/problem+json" },
  });

const list = (names: string[]) =>
  new Response(
    JSON.stringify({
      items: names.map((name) => ({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "DataSource",
        metadata: { name, namespace: "helsinki" },
        spec: {},
      })),
    }),
    { status: 200, headers: { "content-type": "application/json" } },
  );

describe("the resource picker", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("says why when the list cannot be loaded, and offers to ask again", async () => {
    const fetchMock = vi.fn(async () => refusal("you may not read the data sources of helsinki"));
    vi.stubGlobal("fetch", fetchMock);

    show();

    const said = await screen.findByRole("alert");
    expect(said).toHaveTextContent("you may not read the data sources of helsinki");
    // Not an empty project: the option that stands in for the list never claims there is nothing.
    expect(said.textContent).not.toContain(en.form.choose);
    expect(fetchMock).toHaveBeenCalledTimes(1);

    await userEvent.click(within(said).getByRole("button", { name: en.form.listRetry }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
  });

  it("shows the reason in the language the form is in", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => refusal("the Portal did not answer", 503)));
    await i18n.changeLanguage("sk");

    show();

    const said = await screen.findByRole("alert");
    // The sentence is the locale's; the reason inside it is the server's, never translated.
    expect(said).toHaveTextContent("Zoznam sa nepodarilo načítať");
    expect(said).toHaveTextContent("the Portal did not answer");
    await i18n.changeLanguage("en");
  });

  it("says no results for a project that really has none, and what to do next (T-6163)", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => list([])));

    show();

    await waitFor(() =>
      expect(screen.getByRole("combobox")).toHaveTextContent(en.form.noResults),
    );
    expect(screen.getByRole("combobox")).toHaveAccessibleDescription(en.form.noneYet);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("names where a context space is created when the project holds none (T-6163)", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => list([])));

    show({ options: { plural: "spaces", project: "helsinki", empty: "form.noSpaces" } });

    await waitFor(() => expect(screen.getByText(en.form.noSpaces)).toBeInTheDocument());
    expect(screen.getByRole("combobox")).toHaveAccessibleDescription(en.form.noSpaces);
  });

  it("lists what the project has", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => list(["gbfs", "air-feed"])));

    show();

    await waitFor(() => expect(screen.getByRole("option", { name: "gbfs" })).toBeInTheDocument());
    expect(screen.getByRole("option", { name: "air-feed" })).toBeInTheDocument();
    expect(screen.getByRole("combobox")).toHaveTextContent(en.form.choose);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
