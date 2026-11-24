/**
 * T-5122: a visitor's public page boots its own small router (PublicApp, publicRouter.tsx) in the
 * public frame, with a sign-in button that comes back to the same address.
 */
import { render, screen, waitFor } from "@testing-library/react";
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

function visit(path: string, me: { body: unknown; status: number } = { body: { title: "Unauthorized" }, status: 401 }) {
  window.history.pushState({}, "", path);
  vi.spyOn(globalThis, "fetch").mockImplementation((input: RequestInfo | URL) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    if (url.includes("/auth/me")) return Promise.resolve(jsonResponse(me.body, me.status));
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

  it.each([
    "/catalogue",
    "/catalogue/air-quality",
    "/catalogue/endpoint/hel-air",
    "/glossary",
    "/connect/helsinki/air",
    "/v/hel-air",
    "/d/hel-air",
    "/f/hel-report",
  ])("draws %s in the public frame, a missing one as its own page's answer", async (path) => {
    visit(path);
    expect(await screen.findByRole("heading", { level: 1 }, { timeout: 5000 })).toBeInTheDocument();
    expect(screen.getAllByRole("main").length).toBeGreaterThan(0);
    expect(window.location.pathname).toBe(path);
  });

  it("hands an address no public page claims to the full Portal by a reload, drawing only a busy page", async () => {
    visit("/catalogue/a/b/c");
    await waitFor(() => expect(document.querySelector('main[aria-busy="true"]')).not.toBeNull());
    expect(screen.queryByRole("heading", { level: 1 })).toBeNull();
    expect(screen.queryByRole("navigation", { name: "About this site" })).toBeNull();
  });

  it("loads the full Portal, with its shell, for a signed-in person on a public page", async () => {
    visit("/glossary", { body: { subject: "s", username: "jana.kovacova", roles: ["domain-editor"] }, status: 200 });
    expect(await screen.findByRole("heading", { level: 1 }, { timeout: 15000 })).toBeInTheDocument();
    // The public frame's sign-in button is the visitor's; the person's own Portal draws none.
    await waitFor(() => expect(screen.queryByRole("button", { name: "Sign in" })).toBeNull(), { timeout: 15000 });
    expect(screen.queryByRole("navigation", { name: "About this site" })).toBeNull();
  }, 30000);

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
