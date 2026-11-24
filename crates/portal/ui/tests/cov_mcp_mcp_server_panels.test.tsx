/**
 * T-7589: Coverage for McpServerPanels ToolPreview health status branches, empty states, and CopyValue clipboard failure.
 */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { CopyValue, ToolPreview } from "../src/pages/mcp/McpServerPanels";
import type { MemberChoice } from "../src/pages/mcp/mcp";

describe("McpServerPanels coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("displays refused and failed states in ToolPreview health badges", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname === "/api/endpoint/refused-ep/mcp") {
          return new Response(JSON.stringify({ status: 403, title: "Forbidden", detail: "forbidden" }), {
            status: 403,
            headers: { "Content-Type": "application/problem+json" },
          });
        }
        if (url.pathname === "/api/endpoint/failed-ep/mcp") {
          return new Response(JSON.stringify({ status: 500, title: "Server Error" }), {
            status: 500,
            headers: { "Content-Type": "application/problem+json" },
          });
        }
        if (url.pathname === "/api/endpoint/rpc-error-ep/mcp") {
          return new Response(JSON.stringify({ jsonrpc: "2.0", id: 1, error: { message: "Internal RPC error" } }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const members: MemberChoice[] = [
      { project: "helsinki", name: "refused-ep", title: "Refused", slug: "refused-ep", audience: "public", servesMcp: true },
      { project: "helsinki", name: "failed-ep", title: "Failed", slug: "failed-ep", audience: "public", servesMcp: true },
      { project: "helsinki", name: "rpc-error-ep", title: "RpcError", slug: "rpc-error-ep", audience: "public", servesMcp: true },
    ];

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <ToolPreview members={members} />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    expect(await screen.findByText(en.mcp.health.refused.replace("{status}", "403"))).toBeInTheDocument();
    expect(await screen.findByText(en.mcp.health.failed.replace("{reason}", "HTTP 500"))).toBeInTheDocument();
    expect(await screen.findByText(en.mcp.health.failed.replace("{reason}", "Internal RPC error"))).toBeInTheDocument();
  });

  it("renders CopyValue with code pre block and inline code", () => {
    const { rerender } = render(
      <I18nextProvider i18n={i18n}>
        <CopyValue label="Snippet" value="console.log('hi')" code />
      </I18nextProvider>,
    );

    const pre = screen.getByLabelText("Snippet");
    expect(pre.tagName.toLowerCase()).toBe("pre");

    rerender(
      <I18nextProvider i18n={i18n}>
        <CopyValue label="Snippet" value="plain" code={false} />
      </I18nextProvider>,
    );
    expect(screen.getByText("plain").tagName.toLowerCase()).toBe("code");
  });

  it("renders ToolPreview with no tools message when all probes finish with empty tools", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: 1, result: { tools: [] } }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }),
    );

    const members: MemberChoice[] = [
      { project: "helsinki", name: "empty-ep", title: "Empty", slug: "empty-ep", audience: "public", servesMcp: true },
    ];

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <ToolPreview members={members} />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    expect(await screen.findByText(en.mcp.preview.noTools)).toBeInTheDocument();
  });
});
