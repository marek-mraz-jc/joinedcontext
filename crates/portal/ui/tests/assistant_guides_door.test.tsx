import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useOpendataHost } from "../src/components/endpoints/AssistantGuides";

const original = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = original;
});

describe("the assistant guides' open data host (T-7167, viewer smoke 404)", () => {
  it("asks the addresses door, which answers a project's viewer, never jc_organization_read", async () => {
    const asked: string[] = [];
    // A plain function, not a mock: setup.ts translates a mock's door calls to their REST
    // address, and this test reads the address the page asks.
    globalThis.fetch = async (input: RequestInfo | URL) => {
      const url = decodeURIComponent(
        input instanceof Request ? input.url : String(input),
      );
      asked.push(url);
      if (
        !url.includes("JoinedContext.Portal.portal_organization_address_list()")
      ) {
        return new Response("{}", { status: 404 });
      }
      const value = {
        items: [
          {
            organization: "zilina",
            title: "Žilina",
            addresses: [
              { kind: "opendata", url: "https://opendata.zilina.example/" },
            ],
          },
        ],
      };
      return new Response(JSON.stringify({ value }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    };
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => useOpendataHost(), { wrapper });
    await waitFor(() =>
      expect(result.current).toBe("https://opendata.zilina.example/"),
    );
    expect(asked.some((url) => url.includes("jc_organization_read"))).toBe(
      false,
    );
  });
});
