/**
 * The Portal shows no Federation page and no federation playground (UI-28): an old link lands
 * somewhere real, and the sidebar offers neither.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", roles: ["portal-viewer"] };
const EMPTY_LIST = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };
const PROJECTS = { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: "helsinki" }] };

function renderAt(path: string) {
  window.history.pushState({}, "", path);
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const body = url.includes("/auth/me")
        ? IDENTITY
        : url.endsWith("/api/v1/projects")
          ? PROJECTS
          : EMPTY_LIST;
      return Promise.resolve(
        new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }),
      );
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("no federation page (UI-28)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("sends an old Federation link to the project's context spaces", async () => {
    renderAt("/projects/helsinki/federation");
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/spaces"));
    const nav = await screen.findByRole("navigation", { name: "Main navigation" });
    expect(within(nav).queryByRole("link", { name: /federation/i })).not.toBeInTheDocument();
    expect(within(nav).queryByRole("link", { name: /playground/i })).not.toBeInTheDocument();
  });

  it("sends an old playground link away from the playground", async () => {
    renderAt("/playground");
    await waitFor(() => expect(window.location.pathname).not.toBe("/playground"));
  });
});
