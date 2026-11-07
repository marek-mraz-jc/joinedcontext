/**
 * T-1819: the widget that picks one of the project's manifests, against the UI contract
 * (UI-04, UI-15, UI-16, UI-44, UI-48). It had no test of its own.
 *
 * Its whole argument is that a list which could not be asked for is not an empty project
 * (T-1486): a 403 or a 500 used to read as "No results" in every form that picks a resource, so
 * nobody could tell a project with nothing in it from one they may not read. The refusal is in
 * words beside the control now, with a retry, and the control is the shared `Select`.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import type { JsonSchema, UiSchema } from "../src/components/forms/types";
import { expectNoRawKeys, expectNoViolations } from "./checks";

const SCHEMA: JsonSchema = {
  type: "object",
  properties: { endpoint: { type: "string", title: "Endpoint" } },
};

const UI: UiSchema = {
  endpoint: {
    "ui:widget": "resourcePicker",
    "ui:options": { plural: "endpoints", project: "helsinki" },
  },
};

const ENDPOINTS = [
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "helsinki-air", title: { en: "Air quality", sk: "Kvalita ovzdušia" } },
    spec: {},
  },
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "helsinki-bikes" },
    spec: {},
  },
];

/** The list as the API answers it, or the status it refuses with. */
function stub(answer: unknown[] | number) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      calls.push(url);
      return Promise.resolve(
        typeof answer === "number"
          ? new Response(
              JSON.stringify({ type: "about:blank", status: answer, title: "Forbidden", detail: "no read on endpoints" }),
              { status: answer, headers: { "Content-Type": "application/problem+json" } },
            )
          : new Response(
              JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: answer }),
              { status: 200, headers: { "Content-Type": "application/json" } },
            ),
      );
    }),
  );
  return calls;
}

function show() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <SchemaForm schema={SCHEMA} uiSchema={UI} onSubmit={() => {}} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const picker = () => screen.getByLabelText(/Endpoint/) as HTMLSelectElement;

describe("the resource picker against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("offers the project's manifests by their title, with the name as the value", async () => {
    stub(ENDPOINTS);
    const { container } = show();

    await waitFor(() => expect(screen.getByRole("option", { name: "Air quality" })).toBeInTheDocument());
    // A manifest with no title is offered by its name rather than blank.
    expect(screen.getByRole("option", { name: "helsinki-bikes" })).toBeInTheDocument();
    expect(picker().tagName).toBe("SELECT");
    // The shared Select, so the border, the ring and the disabled state come from one place.
    expect(picker().className).toContain("focus-ring");
    await expectNoViolations(container);
  });

  it("keeps the value the form holds even before the list arrives", async () => {
    stub(ENDPOINTS);
    show();
    expect(picker().value).toBe("");
    await waitFor(() => expect(picker().options.length).toBeGreaterThan(1));
  });

  it("says a list it may not read is refused, and not that the project is empty", async () => {
    stub(403);
    const { container } = show();

    const refusal = await screen.findByRole("alert");
    expect(refusal).toHaveTextContent("no read on endpoints");
    // UI-44: the reason is in words beside the control, not a silent "No results".
    expect(refusal).toHaveTextContent(i18n.t("form.listRetry"));
    await expectNoViolations(container);
  });

  it("asks again from the keyboard when the list failed", async () => {
    const calls = stub(403);
    const user = userEvent.setup();
    show();

    await screen.findByRole("alert");
    const before = calls.length;
    const retry = screen.getByRole("button", { name: i18n.t("form.listRetry") });
    retry.focus();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(calls.length).toBeGreaterThan(before));
  });

  it("has nothing to offer when the project holds none, and says so", async () => {
    stub([]);
    show();
    await waitFor(() => expect(screen.getByRole("option", { name: i18n.t("form.noResults") })).toBeInTheDocument());
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it.each(SUPPORTED_LOCALES)("writes the picker's own words in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    stub(ENDPOINTS);
    const { container } = show();

    await waitFor(() =>
      expect(screen.getByRole("option", { name: i18n.t("form.choose") })).toBeInTheDocument(),
    );
    expectNoRawKeys(container);
  });
});
