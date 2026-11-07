/**
 * T-5122: a visitor's public page boots its own small router (PublicApp, publicRouter.tsx) in the
 * public frame, with a sign-in button that comes back to the same address.
 */
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PublicApp } from "../src/PublicApp";
import { PublicFrame } from "../src/components/layout/PublicFrame";
import { problemOf, useThemeLabel } from "../src/pages/catalogue/labels";
import { ApiError } from "../src/api/client";
import { jsonResponse } from "./pageHarness";

afterEach(() => {
  vi.restoreAllMocks();
  window.history.pushState({}, "", "/");
});

function visit(path: string) {
  window.history.pushState({}, "", path);
  vi.spyOn(globalThis, "fetch").mockImplementation((input: RequestInfo | URL) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    if (url.includes("/auth/me")) return Promise.resolve(jsonResponse({ title: "Unauthorized" }, 401));
    return Promise.resolve(jsonResponse({}, 404));
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PublicApp />
    </QueryClientProvider>,
  );
}

describe("the public entry", () => {
  it("draws the accessibility statement in the public frame for a visitor", async () => {
    visit("/accessibility");
    expect(await screen.findByRole("heading", { level: 1 })).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "About this site" })).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Sign in" })).toBeEnabled();
    expect(screen.getByRole("main")).toBeInTheDocument();
    expect(PublicFrame).toBeTypeOf("function");
  });

  it("names a theme by its words and a failed read by the server's detail", () => {
    let label: ((code: string, fallback?: string) => string) | undefined;
    function Probe() {
      label = useThemeLabel();
      return null;
    }
    render(<Probe />);
    expect(label?.("XX-unknown", "Fallback")).toBe("Fallback");
    expect(problemOf(new Error("x"), "It failed")).toBe("It failed");
    expect(problemOf(new ApiError(500, "boom", { type: "about:blank", title: "Unavailable", status: 500, detail: "The catalogue is down" }), "It failed")).toBe("The catalogue is down");
  });
});
