import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { queryKeys } from "../src/api/client";
import { digestOf } from "../src/api/digest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourceFormDialog } from "../src/components/ResourceFormDialog";
import type { JsonSchema } from "../src/components/forms/types";
import { expectOpen } from "./checks";

interface Space {
  name?: string;
  isSandbox?: boolean;
}

const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name"],
  properties: {
    name: { type: "string", title: "Name" },
    isSandbox: { type: "boolean", title: "Sandbox", default: false },
  },
};

const SOURCE = {
  toManifest: ({ name, ...spec }: Space) => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: name ?? "", namespace: "helsinki" },
    spec,
  }),
  fromManifest: (manifest: unknown) => {
    const m = manifest as { metadata?: { name?: string }; spec?: object };
    return { ...(m.spec ?? {}), name: m.metadata?.name } as Space;
  },
};

class SilentEventSource {
  addEventListener() {}
  removeEventListener() {}
  close() {}
}

function Harness() {
  const [form, setForm] = useState<Space | undefined>(undefined);
  return (
    <ResourceFormDialog<Space>
      kind="ContextSpace"
      open
      onOpenChange={() => {}}
      title="Add a context space"
      description="A space the assistant drafted"
      project="helsinki"
      draftKind="ContextSpace"
      draftName="assistant-space"
      plural="spaces"
      schema={SCHEMA}
      source={SOURCE}
      formData={form}
      onChange={setForm}
      submitLabel="Propose"
      onSubmit={() => {}}
    />
  );
}

describe("a draft another hand wrote, without the schema's defaults (T-2624, PF-57)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("EventSource", SilentEventSource);
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  // The store keeps a draft's verdict across a save, so the save the debounce sends while the
  // check is in flight answers with what the store held before it: nothing, or the assistant's
  // own check of the manifest it wrote, which the form's defaults have since moved.
  const drafted = SOURCE.toManifest({ name: "assistant-space" });
  const before = {
    no: null,
    "the assistant's": {
      ok: true,
      findings: [],
      checkedAt: "2026-09-22T08:59:00Z",
      inputDigest: digestOf(drafted),
    },
  };

  it.each(Object.entries(before))("keeps its one check fresh when the store held %s verdict, so Propose opens and stays open", async (_, held) => {
    const checked: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const request = input as Request;
        const url = request.url;
        if (url.includes("/api/v1/branding")) {
          return new Response(JSON.stringify({ validation: "strict" }), { status: 200 });
        }
        if (request.method === "POST" && url.includes("dryRun=All")) {
          const body = JSON.parse(await request.clone().text()) as Record<string, unknown>;
          delete body.draft;
          checked.push(digestOf(body));
          const verdict = { ok: true, findings: [], checkedAt: new Date().toISOString(), inputDigest: digestOf(body) };
          return new Response(JSON.stringify({ valid: true, verdict }), { status: 200 });
        }
        const draft = (manifest: unknown, version: number) =>
          new Response(
            JSON.stringify({
              project: "helsinki",
              kind: "ContextSpace",
              name: "assistant-space",
              manifest,
              verdict: held,
              touchedBy: "assistant",
              touchedKind: "agent",
              version,
              updatedAt: "2026-09-22T09:00:00Z",
            }),
            { status: 200 },
          );
        if (url.includes("/drafts/ContextSpace/assistant-space")) {
          if (request.method === "PUT") {
            const body = JSON.parse(await request.clone().text()) as { manifest: unknown };
            return draft(body.manifest, 2);
          }
          return draft(drafted, 1);
        }
        return new Response(JSON.stringify({}), { status: 200 });
      }),
    );

    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <Harness />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    await waitFor(() => expect(screen.getByLabelText(/Name/)).toHaveValue("assistant-space"));
    fireEvent.click(screen.getByRole("button", { name: en.form.check }));
    const propose = await screen.findByRole("button", { name: "Propose" });
    await waitFor(() => expectOpen(propose));
    // And it stays open: nothing the form fills in after the check moves the draft under it.
    await new Promise((resolve) => setTimeout(resolve, 1200));
    expectOpen(propose);
    expect(checked).toHaveLength(1);
  });
});

// PF-58, PF-70: before the click the dialog says what proposing leads to for this person.
describe("what proposing leads to, said in the dialog", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("EventSource", SilentEventSource);
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  // The page that opens the form has read the person's permissions already; the dialog reads them.
  function withVerbs(verbs: string[]) {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({}), { status: 200 })));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const grants = [{ binding: "b", role: "r", scope: "organization", rule: { kinds: ["ContextSpace"], verbs } }];
    client.setQueryData(queryKeys.permissions("helsinki"), { bootstrap: false, project: "helsinki", grants });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <Harness />
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  // T-6105: approved on propose is the normal case, and the form says nothing about it.
  it("an administrator of the kind hears nothing", async () => {
    withVerbs(["propose", "approve", "delete"]);
    await waitFor(() => expect(screen.getByRole("button", { name: en.form.check })).toBeInTheDocument());
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByTestId("propose-standing")).toBeNull();
    expect(screen.queryByText(/Approved as soon as you propose it/)).toBeNull();
  });

  it("an editor hears nothing new", async () => {
    withVerbs(["propose"]);
    await waitFor(() => expect(screen.getByRole("button", { name: en.form.check })).toBeInTheDocument());
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByTestId("propose-standing")).toBeNull();
  });
});
