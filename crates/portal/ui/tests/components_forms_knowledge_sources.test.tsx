/**
 * T-7585: the deployment's sources picker: ticking and unticking writes the list, a source the
 * manifest names that no project offers stays to be taken away, and a failed list can be asked again.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { FieldProps } from "@rjsf/utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { queryKeys } from "../src/api/client";
import { KnowledgeSourcesField } from "../src/components/forms/widgets/KnowledgeSourcesField";
import { FormProjectContext } from "../src/components/forms/widgets/ModelWidgets";

const source = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "KnowledgeSource",
  metadata: { name, namespace: "helsinki" },
  spec: { source: "website", visibility: "public" },
});

function propsOf(formData: unknown, onChange = vi.fn()): FieldProps {
  return {
    formData,
    onChange,
    disabled: false,
    readonly: false,
    required: false,
    schema: { type: "array", title: "Sources" },
    uiSchema: {},
    rawErrors: [],
    fieldPathId: { $id: "root_sources", path: ["sources"] },
  } as unknown as FieldProps;
}

function show(props: FieldProps, seed: boolean) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  if (seed) {
    client.setQueryData(queryKeys.projects(), { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: "helsinki" }] });
    client.setQueryData(queryKeys.list("helsinki", "knowledgesources"), {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "List",
      items: [source("web"), source("news")],
    });
  }
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <FormProjectContext.Provider value="helsinki">
          <KnowledgeSourcesField {...props} />
        </FormProjectContext.Provider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the sources a deployment answers from", () => {
  it("adds a ticked source to the list and writes nothing but the list at its path", async () => {
    const onChange = vi.fn();
    show(propsOf(["web"], onChange), true);
    await userEvent.click(await screen.findByRole("checkbox", { name: /news/ }));
    expect(onChange).toHaveBeenCalledWith(["web", "news"], ["sources"]);
  });

  it("writes no list at all once the last source is unticked", async () => {
    const onChange = vi.fn();
    show(propsOf(["web"], onChange), true);
    await userEvent.click(await screen.findByRole("checkbox", { name: /web/ }));
    expect(onChange).toHaveBeenCalledWith(undefined, ["sources"]);
  });

  it("keeps a source no project offers visible, and takes it away on request", async () => {
    const onChange = vi.fn();
    show(propsOf(["gone", "web"], onChange), true);
    const group = await screen.findByRole("group", { name: en.knowledge.sourcesPicker.unknown });
    await userEvent.click(within(group).getByRole("checkbox", { name: "gone" }));
    expect(onChange).toHaveBeenCalledWith(["web"], ["sources"]);
  });

  it("says each list that failed and asks the server again on Retry", async () => {
    const fetched = vi.fn(
      async () =>
        new Response(JSON.stringify({ title: "Boom", status: 500, detail: "the mirror is not ready" }), {
          status: 500,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );
    vi.stubGlobal("fetch", fetched);
    show(propsOf([]), false);
    const retries = await screen.findAllByRole("button", { name: en.form.listRetry });
    expect(retries.length).toBeGreaterThanOrEqual(2);
    const before = fetched.mock.calls.length;
    for (const retry of retries) await userEvent.click(retry);
    await waitFor(() => expect(fetched.mock.calls.length).toBeGreaterThan(before));
  });
});
