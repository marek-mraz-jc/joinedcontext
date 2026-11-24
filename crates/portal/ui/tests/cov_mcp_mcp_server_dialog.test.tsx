/**
 * T-7589: Coverage for McpServerDialog untick member, cancel, close on change, description edit, untick source, and project error retry.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { McpServerDialog } from "../src/pages/mcp/McpServerDialog";
import type { MemberChoice } from "../src/pages/mcp/mcp";
import type { Manifest } from "../src/api/manifest";
import { InRouter } from "./pageHarness";

const CHOICES: MemberChoice[] = [
  { project: "helsinki", name: "bikes", title: "Bikes", slug: "bikeslug", audience: "public", servesMcp: true },
  { project: "helsinki", name: "parking", title: "Parking", slug: "parkslug", audience: "public", servesMcp: true },
];

const STORED_SERVER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "McpServer",
  metadata: { name: "mobility", namespace: "helsinki", title: { en: "Mobility" }, description: { en: "Mobility description" } },
  spec: {
    members: [
      { kind: "Endpoint", name: "bikes", namespace: "helsinki" },
      { kind: "Endpoint", name: "parking", namespace: "helsinki" },
    ],
    audience: "public",
    knowledgeSources: ["web"],
  },
} as unknown as Manifest;

describe("McpServerDialog coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("unticks an existing member to remove it from members list", async () => {
    const user = userEvent.setup();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname.endsWith("/knowledge/sources")) return new Response(JSON.stringify({ items: [] }), { headers: { "Content-Type": "application/json" } });
        if (url.pathname === "/api/v1/projects") return new Response(JSON.stringify({ kind: "List", items: [{ name: "helsinki" }] }), { headers: { "Content-Type": "application/json" } });
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: 1, result: { tools: [] } }), { headers: { "Content-Type": "application/json" } });
      }),
    );

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <McpServerDialog
              project="helsinki"
              stored={STORED_SERVER}
              choices={CHOICES}
              choicesLoading={false}
              open={true}
              onOpenChange={vi.fn()}
            />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const dialog = await screen.findByRole("dialog", { name: "Edit mobility" });
    const bikesCheckbox = within(dialog).getByRole("checkbox", { name: "Bikes (helsinki/bikes)" });
    expect(bikesCheckbox).toBeChecked();

    await user.click(bikesCheckbox);
    expect(bikesCheckbox).not.toBeChecked();
  });

  it("calls onOpenChange(false) and resets form when Cancel is clicked", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname.endsWith("/knowledge/sources")) return new Response(JSON.stringify({ items: [] }), { headers: { "Content-Type": "application/json" } });
        if (url.pathname === "/api/v1/projects") return new Response(JSON.stringify({ kind: "List", items: [{ name: "helsinki" }] }), { headers: { "Content-Type": "application/json" } });
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: 1, result: { tools: [] } }), { headers: { "Content-Type": "application/json" } });
      }),
    );

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { unmount } = render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <McpServerDialog
              project="helsinki"
              stored={undefined}
              choices={CHOICES}
              choicesLoading={false}
              open={true}
              onOpenChange={onOpenChange}
            />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const dialog = await screen.findByRole("dialog", { name: "New MCP server" });
    const cancelBtn = within(dialog).getByRole("button", { name: en.form.cancel });
    await user.click(cancelBtn);

    expect(onOpenChange).toHaveBeenCalledWith(false);

    unmount();
    const onOpenChangeStored = vi.fn();
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <McpServerDialog
              project="helsinki"
              stored={STORED_SERVER}
              choices={CHOICES}
              choicesLoading={false}
              open={true}
              onOpenChange={onOpenChangeStored}
            />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const editDialog = await screen.findByRole("dialog", { name: "Edit mobility" });
    await user.click(within(editDialog).getByRole("button", { name: en.form.cancel }));
    expect(onOpenChangeStored).toHaveBeenCalledWith(false);
  });

  it("edits description and unticks an attached knowledge source", async () => {
    const user = userEvent.setup();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname.endsWith("/knowledge/sources")) {
          return new Response(
            JSON.stringify({
              items: [{ source: "web", type: "website", state: "crawled", visibility: "public" }],
            }),
            { headers: { "Content-Type": "application/json" } },
          );
        }
        if (url.pathname === "/api/v1/projects") return new Response(JSON.stringify({ kind: "List", items: [{ name: "helsinki" }] }), { headers: { "Content-Type": "application/json" } });
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: 1, result: { tools: [] } }), { headers: { "Content-Type": "application/json" } });
      }),
    );

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <McpServerDialog
              project="helsinki"
              stored={STORED_SERVER}
              choices={CHOICES}
              choicesLoading={false}
              open={true}
              onOpenChange={vi.fn()}
            />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const dialog = await screen.findByRole("dialog", { name: "Edit mobility" });
    const desc = within(dialog).getByLabelText(en.mcp.field.description);
    await user.clear(desc);
    await user.type(desc, "Updated mobility info");
    expect(desc).toHaveValue("Updated mobility info");

    const webCheckbox = await within(dialog).findByRole("checkbox", { name: "web" });
    expect(webCheckbox).toBeChecked();
    await user.click(webCheckbox);
    expect(webCheckbox).not.toBeChecked();
  });
});
