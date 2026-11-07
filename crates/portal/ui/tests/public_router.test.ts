// T-5122: main.tsx boots the small public router for exactly the pages it draws.
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient } from "@tanstack/react-query";
import { createMemoryHistory } from "@tanstack/react-router";
import { createPublicRouter, isPublicPath } from "../src/publicRouter";

afterEach(() => vi.restoreAllMocks());

/** The URLs the router asks for while it matches `path`, before any page chunk draws. */
async function readsOnMatch(path: string): Promise<string[]> {
  const fetch = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("{}", { status: 200, headers: { "content-type": "application/json" } }));
  const router = createPublicRouter(new QueryClient(), createMemoryHistory({ initialEntries: [path] }));
  await router.load();
  return fetch.mock.calls.map(([input]) => (input instanceof Request ? input.url : String(input)));
}

describe("the public entry", () => {
  // T-5165: the read starts as the route matches, beside the page's chunk, not after the chunk drew.
  it("starts the catalogue and dataset reads as the route matches", async () => {
    expect((await readsOnMatch("/catalogue?q=voda")).some((url) => url.includes("/api/v1/catalogue?q=voda"))).toBe(true);
    vi.restoreAllMocks();
    expect((await readsOnMatch("/catalogue/bb-pocasie")).some((url) => url.includes("/api/v1/catalogue/datasets/bb-pocasie"))).toBe(true);
    vi.restoreAllMocks();
    expect(await readsOnMatch("/accessibility")).toEqual([]);
  });

  it("claims the pages anybody may open and nothing a person signs in for", () => {
    for (const path of ["/catalogue", "/catalogue/", "/catalogue/bb-pocasie", "/glossary", "/accessibility", "/v/abc", "/f/abc", "/d/abc", "/connect/p/n"]) {
      expect(isPublicPath(path), path).toBe(true);
    }
    for (const path of ["/", "/login", "/projects/p/home", "/catalogue/a/b", "/organization", "/v", "/endpoints"]) {
      expect(isPublicPath(path), path).toBe(false);
    }
  });

  it("routes every path it claims, so a claimed page never reloads into itself", () => {
    const router = createPublicRouter(new QueryClient());
    for (const path of ["/catalogue", "/catalogue/bb-pocasie", "/glossary", "/accessibility", "/v/abc", "/f/abc", "/d/abc", "/connect/p/n"]) {
      const matches = router.matchRoutes(path, {});
      expect(matches.at(-1)?.routeId, path).not.toBe("__root__");
    }
  });
});
