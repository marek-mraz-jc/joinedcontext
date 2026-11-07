/**
 * T-6175, T-6188: a kind's list as the server pages and searches it. The grid's search reaches the
 * API as `q` once typing pauses, Show more reads the next cursor, and the total is the last page's.
 */
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { GRID_PAGE, useKindPages } from "../src/routes/kindPages";
import { json } from "./page_contract";

function manifest(name: string) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "Mapping", metadata: { name, namespace: "helsinki" }, spec: {} };
}

const asked: URL[] = [];

function answering() {
  asked.length = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(input instanceof Request ? input.url : String(input), window.location.origin);
      asked.push(url);
      const cursor = url.searchParams.get("cursor");
      const q = url.searchParams.get("q");
      if (q) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [manifest(`found-${q}`)], totalCount: 1 });
      return cursor
        ? json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [manifest("second")], totalCount: 2 })
        : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [manifest("first")], nextCursor: "c2", totalCount: 2 });
    }),
  );
}

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{children}</QueryClientProvider>
);

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a kind's list read page by page", () => {
  it("asks for a counted page, then the next cursor on Show more", async () => {
    answering();
    const { result } = renderHook(() => useKindPages("helsinki", "mappings"), { wrapper });
    await waitFor(() => expect(result.current.items.map((m) => m.metadata.name)).toEqual(["first"]));
    expect(asked[0]?.pathname).toBe("/api/v1/projects/helsinki/mappings");
    expect(asked[0]?.searchParams.get("limit")).toBe(String(GRID_PAGE));
    expect(asked[0]?.searchParams.get("count")).toBe("true");
    expect(asked[0]?.searchParams.has("q")).toBe(false);
    expect(result.current.server.total).toBe(2);
    expect(result.current.server.hasMore).toBe(true);

    act(() => result.current.server.loadMore());
    await waitFor(() => expect(result.current.items.map((m) => m.metadata.name)).toEqual(["first", "second"]));
    expect(asked.at(-1)?.searchParams.get("cursor")).toBe("c2");
    expect(result.current.server.hasMore).toBe(false);
  });

  it("sends the trimmed search as q once typing pauses", async () => {
    answering();
    const { result } = renderHook(() => useKindPages("helsinki", "mappings"), { wrapper });
    await waitFor(() => expect(result.current.items).toHaveLength(1));
    act(() => result.current.server.onSearch("  bus "));
    await waitFor(() => expect(result.current.items.map((m) => m.metadata.name)).toEqual(["found-bus"]));
    expect(asked.filter((url) => url.searchParams.has("q")).map((url) => url.searchParams.get("q"))).toEqual(["bus"]);
    expect(result.current.server.total).toBe(1);
  });
});
