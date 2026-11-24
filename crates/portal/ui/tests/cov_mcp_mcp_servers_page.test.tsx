/**
 * T-7589: Coverage for McpServersPage error retry, edit/remove actions and dialog closing handlers.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { McpServersPage } from "../src/pages/mcp/McpServersPage";
import { InRouter } from "./pageHarness";

const IDENTITY = { subject: "b7c1e0f4", username: "eva.steward", name: "Eva Steward", roles: [] };

const PERMISSIONS = {
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "steward", binding: "people", scope: "project:helsinki", rule: { kinds: ["McpServer", "Endpoint"], verbs: ["read", "propose", "delete"] } }],
};

const SERVER_ITEM = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "McpServer",
  metadata: {
    name: "mobility",
    namespace: "helsinki",
    title: { en: "Mobility data" },
    description: { en: "Urban mobility endpoints" },
  },
  spec: {
    members: [
      { kind: "Endpoint", name: "bikes", namespace: "helsinki" },
      { kind: "Endpoint", name: "unknown-ep", namespace: "other" },
    ],
    audience: "public",
  },
};

describe("McpServersPage coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("retries fetching servers when PageFailed retry button is clicked", async () => {
    let attempts = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname.endsWith("/mcpservers")) {
          attempts++;
          if (attempts === 1) {
            return new Response(JSON.stringify({ title: "Failed", status: 500, detail: "DB error" }), {
              status: 500,
              headers: { "Content-Type": "application/problem+json" },
            });
          }
          return new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        if (url.pathname.endsWith("/auth/me")) return new Response(JSON.stringify(IDENTITY), { headers: { "Content-Type": "application/json" } });
        if (url.pathname.endsWith("/permissions/me")) return new Response(JSON.stringify(PERMISSIONS), { headers: { "Content-Type": "application/json" } });
        if (url.pathname.endsWith("/branding")) return new Response(JSON.stringify({ instanceName: "test" }), { headers: { "Content-Type": "application/json" } });
        if (url.pathname === "/api/v1/projects") return new Response(JSON.stringify({ kind: "List", items: [{ name: "helsinki" }] }), { headers: { "Content-Type": "application/json" } });
        return new Response(JSON.stringify({ kind: "List", items: [] }), { headers: { "Content-Type": "application/json" } });
      }),
    );

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <McpServersPage project="helsinki" />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const retryButton = await screen.findByRole("button", { name: /try again|retry/i });
    await userEvent.click(retryButton);

    await waitFor(() => expect(attempts).toBeGreaterThanOrEqual(2));
    expect(await screen.findByText(en.mcp.empty)).toBeInTheDocument();
  });

  it("opens edit dialog on onEdit and resets editing state when closed", async () => {
    const user = userEvent.setup();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname.endsWith("/mcpservers")) {
          return new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [SERVER_ITEM] }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        if (url.pathname.endsWith("/auth/me")) return new Response(JSON.stringify(IDENTITY), { headers: { "Content-Type": "application/json" } });
        if (url.pathname.endsWith("/permissions/me")) return new Response(JSON.stringify(PERMISSIONS), { headers: { "Content-Type": "application/json" } });
        if (url.pathname.endsWith("/branding")) return new Response(JSON.stringify({ instanceName: "test", domain: "dev.example" }), { headers: { "Content-Type": "application/json" } });
        if (url.pathname === "/api/v1/projects") return new Response(JSON.stringify({ kind: "List", items: [{ name: "helsinki" }] }), { headers: { "Content-Type": "application/json" } });
        if (url.pathname.endsWith("/endpoints")) {
          return new Response(
            JSON.stringify({
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "List",
              items: [{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Endpoint", metadata: { name: "bikes", namespace: "helsinki", title: { en: "Bikes" } }, spec: { slug: "bikeslug", audience: "public" } }],
            }),
            { headers: { "Content-Type": "application/json" } },
          );
        }
        if (url.pathname.endsWith("/knowledge/sources")) return new Response(JSON.stringify({ items: [] }), { headers: { "Content-Type": "application/json" } });
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: 1, result: { tools: [] } }), { headers: { "Content-Type": "application/json" } });
      }),
    );

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <McpServersPage project="helsinki" />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    expect(await screen.findByText("Urban mobility endpoints")).toBeInTheDocument();
    expect(screen.getByText("Members you may not read, kept as they are: 1")).toBeInTheDocument();

    const editBtn = await screen.findByRole("button", { name: "Edit mobility" });
    await user.click(editBtn);

    const dialog = await screen.findByRole("dialog", { name: "Edit mobility" });
    expect(dialog).toBeInTheDocument();

    const cancelBtn = within(dialog).getByRole("button", { name: en.form.cancel });
    await user.click(cancelBtn);

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit mobility" })).toBeNull());
  });
});
