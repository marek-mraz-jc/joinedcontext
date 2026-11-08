// covers (T-2137, the module gate in gate_modules.test.ts): src/components/forms/ResourceFormFooter.tsx.
/**
 * jc-admin-ui, AG-61: a form that holds a saved draft can throw it away, the call the
 * `jc_draft_drop` operation makes. It asks once more first, closes the form once the server took
 * it, and says the server's refusal beside the button.
 */
import { useState } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourceFormDialogBody as ResourceFormDialog } from "../src/components/ResourceFormDialogBody";
import type { JsonSchema } from "../src/components/forms/types";

interface Form {
  name?: string;
  url?: string;
}

const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name"],
  properties: { name: { type: "string", title: "Name" }, url: { type: "string", title: "URL" } },
};

const SOURCE = {
  toManifest: (form: Form) => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: { name: form.name ?? "", namespace: "helsinki" },
    spec: { type: "http", http: { url: form.url ?? "" } },
  }),
  fromManifest: (manifest: unknown) => {
    const m = manifest as { metadata?: { name?: string }; spec?: { http?: { url?: string } } };
    return { name: m.metadata?.name, url: m.spec?.http?.url };
  },
};

const STORED = {
  project: "helsinki",
  kind: "DataSource",
  name: "air-feed",
  manifest: SOURCE.toManifest({ name: "air-feed", url: "https://example.org/air" }),
  touchedBy: "jana",
  touchedKind: "person",
  version: 2,
  updatedAt: "2026-11-08T09:00:00Z",
};

const L = en.drafts.discard;

function serve(options: { draft: boolean; refuse?: boolean }) {
  const deletes: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const req = input as Request;
      const url = new URL(req.url);
      if (url.pathname === "/api/v1/projects/helsinki/drafts/DataSource/air-feed") {
        if (req.method === "DELETE") {
          deletes.push(url.pathname);
          if (options.refuse) {
            return new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "you may not write drafts of helsinki" }), {
              status: 403,
              headers: { "Content-Type": "application/problem+json" },
            });
          }
          return new Response(JSON.stringify({ dropped: true }), { status: 200, headers: { "Content-Type": "application/json" } });
        }
        if (req.method === "GET") {
          return options.draft
            ? new Response(JSON.stringify(STORED), { status: 200, headers: { "Content-Type": "application/json" } })
            : new Response(JSON.stringify({ status: 404 }), { status: 404, headers: { "Content-Type": "application/json" } });
        }
      }
      return new Response("{}", { status: 200, headers: { "Content-Type": "application/json" } });
    }),
  );
  return deletes;
}

function Harness({ onOpenChange }: { onOpenChange: (open: boolean) => void }) {
  const [formData, setFormData] = useState<Form | undefined>(undefined);
  return (
    <ResourceFormDialog<Form>
      open
      onOpenChange={onOpenChange}
      title="Edit Data Source"
      description="Edit draft"
      project="helsinki"
      draftKind="DataSource"
      draftName="air-feed"
      plural="datasources"
      schema={SCHEMA}
      submitLabel="Propose change"
      source={SOURCE}
      formData={formData}
      onChange={setFormData}
      onSubmit={() => {}}
    />
  );
}

function mount(onOpenChange = vi.fn()) {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <Harness onOpenChange={onOpenChange} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return onOpenChange;
}

describe("discarding a saved draft (AG-61, jc_draft_drop)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("EventSource", undefined);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("asks once more, sends one DELETE and closes the form", async () => {
    const deletes = serve({ draft: true });
    const closed = mount();
    await userEvent.click(await screen.findByRole("button", { name: L.ask }));
    expect(deletes, "the first click only asks").toEqual([]);
    const ask = screen.getByRole("group", { name: L.ask });
    expect(ask).toHaveTextContent(L.confirm);
    await userEvent.click(within(ask).getByRole("button", { name: L.yes }));
    await waitFor(() => expect(deletes).toEqual(["/api/v1/projects/helsinki/drafts/DataSource/air-feed"]));
    await waitFor(() => expect(closed).toHaveBeenCalledWith(false));
  });

  it("keeps the draft when the person changes their mind", async () => {
    const deletes = serve({ draft: true });
    mount();
    await userEvent.click(await screen.findByRole("button", { name: L.ask }));
    await userEvent.click(screen.getByRole("button", { name: L.keep }));
    expect(screen.getByRole("button", { name: L.ask })).toBeInTheDocument();
    expect(deletes).toEqual([]);
  });

  it("says the server's refusal beside the button and leaves the form open", async () => {
    serve({ draft: true, refuse: true });
    const closed = mount();
    await userEvent.click(await screen.findByRole("button", { name: L.ask }));
    await userEvent.click(screen.getByRole("button", { name: L.yes }));
    const group = screen.getByRole("group", { name: L.ask });
    expect(await within(group).findByRole("alert")).toHaveTextContent("The draft was not discarded: you may not write drafts of helsinki");
    expect(closed).not.toHaveBeenCalled();
  });

  it("offers nothing to discard when the store holds no draft", async () => {
    serve({ draft: false });
    mount();
    expect(await screen.findByRole("button", { name: "Propose change" })).toBeInTheDocument();
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole("button", { name: L.ask })).toBeNull();
  });
});
