import { useState } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourceFormDialogBody as ResourceFormDialog } from "../src/components/ResourceFormDialogBody";
import { ResourceHistory } from "../src/components/ResourceHistory";
import type { JsonSchema } from "../src/components/forms/types";

// T-3975: an existing resource's editor has a History view of the changes that touched it; a
// merged one's version is loaded into the form to be checked and proposed, never written here.
const perms = vi.hoisted(() => ({ propose: true }));
vi.mock("../src/api/permissions", async (original) => ({
  ...(await original<typeof import("../src/api/permissions")>()),
  usePermissions: () => ({ can: (_kind: string, verb: string) => verb !== "propose" || perms.propose }),
}));
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value }: { value: string }) => <textarea aria-label="manifest-text" value={value} readOnly />,
}));

class QuietEventSource {
  constructor(public url: string) {}
  addEventListener() {}
  removeEventListener() {}
  close() {}
}

const SCHEMA: JsonSchema = {
  type: "object",
  properties: { name: { type: "string", title: "Name" }, mapping: { type: "string", title: "Mapping" } },
};

interface Form {
  name?: string;
  mapping?: string;
}

const source = {
  toManifest: (form: Form) => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Pipeline",
    metadata: { name: form.name, namespace: "helsinki" },
    spec: { mapping: form.mapping },
  }),
  fromManifest: (manifest: unknown) => {
    const m = manifest as { metadata?: { name?: string }; spec?: { mapping?: string } };
    return { name: m.metadata?.name, mapping: m.spec?.mapping };
  },
};

function change(id: string, phase: string, name: string, at: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Change",
    metadata: { name: id, namespace: "helsinki" },
    status: { lane: "yellow", phase, plan: { create: 0, update: 1, delete: 0 } },
    summary: { key: "change.summary.update", params: { kind: "Pipeline", name, fields: 1 } },
    author: { name: "Aino Virtanen" },
    createdAt: at,
    decision: { by: "mikko@hel.fi", at },
  };
}

const requests: string[] = [];
function stubApi() {
  requests.length = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      requests.push(`${request.method} ${url.pathname}${url.search}`);
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
      if (url.pathname.endsWith("/changes/history")) {
        return json({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "ChangeList",
          items: [
            change("chg-00000009", "Merged", "air", "2026-10-11T10:00:00Z"),
            change("chg-00000008", "Rejected", "air", "2026-10-10T10:00:00Z"),
            // The API matches names containing the one asked; this one is another pipeline.
            change("chg-00000007", "Merged", "air-archive", "2026-10-09T10:00:00Z"),
          ],
        });
      }
      if (url.pathname.endsWith("/changes/chg-00000009/manifests")) {
        return json({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          metadata: {},
          items: [
            { apiVersion: "joinedcontext.com/v1alpha1", kind: "Pipeline", metadata: { name: "air", namespace: "helsinki" }, spec: { mapping: "root = this.old" } },
          ],
        });
      }
      return json({ type: "about:blank", title: "Not Found", status: 404, detail: "no draft" }, 404);
    }),
  );
}

function open(seen: (form: Form) => void): void {
  function Harness(): React.JSX.Element {
    const [form, setForm] = useState<Form>({ name: "air", mapping: "root = this" });
    return (
      <ResourceFormDialog<Form>
        open
        onOpenChange={() => {}}
        title="Edit pipeline"
        description="A pipeline"
        schema={SCHEMA}
        formData={form}
        onChange={(next) => {
          seen(next ?? {});
          setForm(next ?? {});
        }}
        submitLabel="Propose"
        source={source}
        project="helsinki"
        draftKind="Pipeline"
        lockedName="air"
        onSubmit={() => {}}
      />
    );
  }
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <Harness />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("a resource's own history in its editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("EventSource", QuietEventSource);
    perms.propose = true;
    stubApi();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("lists the closed changes of this resource alone and loads a merged version into the form", async () => {
    const seen = vi.fn();
    open(seen);
    await userEvent.click(screen.getByRole("tab", { name: en.form.view.history }));
    const rows = await screen.findAllByTestId("resource-history-item");
    expect(rows).toHaveLength(2);
    expect(requests).toContain("GET /api/v1/projects/helsinki/changes/history?page=1&kind=Pipeline&name=air");
    expect(rows[0]).toHaveTextContent("chg-00000009");
    // A rejected change never was a version: nothing to restore on it.
    expect(within(rows[1]).queryByTestId("resource-history-restore")).not.toBeInTheDocument();

    await userEvent.click(within(rows[0]).getByTestId("resource-history-restore"));
    expect(await screen.findByTestId("history-restored")).toHaveTextContent("chg-00000009");
    await waitFor(() => expect(seen).toHaveBeenLastCalledWith({ name: "air", mapping: "root = this.old" }));
    expect(screen.getByRole("tab", { name: en.form.view.form })).toHaveAttribute("aria-selected", "true");
    expect(requests.filter((r) => !r.startsWith("GET "))).toEqual([]);
  });

  it("shows a viewer the history and no restore", async () => {
    perms.propose = false;
    open(vi.fn());
    await userEvent.click(screen.getByRole("tab", { name: en.form.view.history }));
    expect(await screen.findAllByTestId("resource-history-item")).toHaveLength(2);
    expect(screen.queryByTestId("resource-history-restore")).not.toBeInTheDocument();
  });

  it("offers no History for a resource not yet created", () => {
    function New(): React.JSX.Element {
      return (
        <ResourceFormDialog<Form>
          open
          onOpenChange={() => {}}
          title="New pipeline"
          description="A pipeline"
          schema={SCHEMA}
          formData={{}}
          submitLabel="Propose"
          source={source}
          project="helsinki"
          draftKind="Pipeline"
          onSubmit={() => {}}
        />
      );
    }
    render(
      <QueryClientProvider client={new QueryClient()}>
        <I18nextProvider i18n={i18n}>
          <New />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(screen.getByRole("tab", { name: en.form.view.yaml })).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: en.form.view.history })).not.toBeInTheDocument();
  });

  it("says when no change touched it, and that a change wrote no such version", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: [] }), { status: 200 })),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <ResourceHistory project="helsinki" kind="Pipeline" name="fresh" onRestore={vi.fn()} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(await screen.findByText("No closed change touched Pipeline fresh yet.")).toBeInTheDocument();
  });

  it("refuses to load a version the change did not write", async () => {
    const onRestore = vi.fn();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL((input as Request).url);
        const body = url.pathname.endsWith("/manifests")
          ? { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", metadata: {}, items: [] }
          : { apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: [change("chg-00000009", "Merged", "air", "2026-10-11T10:00:00Z")] };
        return new Response(JSON.stringify(body), { status: 200 });
      }),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <ResourceHistory project="helsinki" kind="Pipeline" name="air" onRestore={onRestore} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await userEvent.click(await screen.findByTestId("resource-history-restore"));
    expect(await screen.findByRole("alert")).toHaveTextContent("That change wrote no Pipeline air");
    expect(onRestore).not.toHaveBeenCalled();
  });
});
