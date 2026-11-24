/**
 * T-7589: Coverage for McpElicitationPage and McpConfirmationPage error retry and navigation branches.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { McpConfirmationPage, McpElicitationPage } from "../src/pages/mcp/McpElicitationPage";
import { InRouter } from "./pageHarness";

const ELICITATION_ID = "eli-0a1b2c3d4e5f6a7b";
const CONFIRMATION_ID = "0a1b2c3d4e5f60718293a4b5c6d7e8f9";

describe("McpElicitationPage and McpConfirmationPage coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("retries fetching when ResourcePageFailed retry button is clicked in McpElicitationPage", async () => {
    let callCount = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        callCount++;
        const request = input as Request;
        const url = new URL(request.url ?? String(input), "http://localhost");
        if (url.pathname.includes("/mcp-elicitations/")) {
          if (callCount === 1) {
            return new Response(JSON.stringify({ status: 500, title: "Server Error", detail: "Internal error" }), {
              status: 500,
              headers: { "Content-Type": "application/problem+json" },
            });
          }
          return new Response(
            JSON.stringify({
              id: ELICITATION_ID,
              operation: "jc_resource_delete",
              message: "Delete resource in project 'city'",
              resource: "/projects/city/endpoints/air",
              expiresAt: 1_900_000_000,
              decision: null,
            }),
            { status: 200, headers: { "Content-Type": "application/json" } },
          );
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <McpElicitationPage project="city" id={ELICITATION_ID} />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const retryButton = await screen.findByRole("button", { name: /try again|retry/i });
    expect(retryButton).toBeInTheDocument();
    await userEvent.click(retryButton);

    expect(await screen.findByTestId("mcp-elicitation-message")).toHaveTextContent("Delete resource in project 'city'");
    expect(callCount).toBeGreaterThanOrEqual(2);
  });

  it("navigates home to root when open is false in McpElicitationPage", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const request = input as Request;
        const url = new URL(request.url ?? String(input), "http://localhost");
        if (url.pathname.includes("/mcp-elicitations/")) {
          return new Response(
            JSON.stringify({
              id: ELICITATION_ID,
              operation: "jc_resource_delete",
              message: "Delete resource in project 'city'",
              resource: "/projects/city/endpoints/air",
              expiresAt: 1_900_000_000,
              decision: null,
            }),
            { status: 200, headers: { "Content-Type": "application/json" } },
          );
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <McpElicitationPage project="city" id={ELICITATION_ID} open={false} />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const homeLink = await screen.findByRole("link", { name: en.mcpConfirmation.home });
    expect(homeLink).toHaveAttribute("href", "/");
  });

  it("retries fetching when ResourcePageFailed retry button is clicked in McpConfirmationPage", async () => {
    let callCount = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        callCount++;
        const request = input as Request;
        const url = new URL(request.url ?? String(input), "http://localhost");
        if (url.pathname.includes("/mcp-confirmations/")) {
          if (callCount === 1) {
            return new Response(JSON.stringify({ status: 500, title: "Broker Error", detail: "Broker unreachable" }), {
              status: 500,
              headers: { "Content-Type": "application/problem+json" },
            });
          }
          return new Response(
            JSON.stringify({
              id: CONFIRMATION_ID,
              view: "bb-air",
              tool: "create_entity",
              message: "Create entity in view bb-air",
              expiresAt: 1_900_000_000,
              decision: null,
            }),
            { status: 200, headers: { "Content-Type": "application/json" } },
          );
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <McpConfirmationPage id={CONFIRMATION_ID} />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );

    const retryButton = await screen.findByRole("button", { name: /try again|retry/i });
    expect(retryButton).toBeInTheDocument();
    await userEvent.click(retryButton);

    expect(await screen.findByTestId("mcp-elicitation-message")).toHaveTextContent("Create entity in view bb-air");
    expect(callCount).toBeGreaterThanOrEqual(2);
  });
});
