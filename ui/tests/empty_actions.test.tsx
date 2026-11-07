import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

// Every list empty, the caller a bootstrap administrator: the empty state of each page offers
// the way to its first item, the same control the header has (T-1381).
function renderEmpty(path: string, bootstrap = true) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const body = url.includes("/auth/me")
        ? { subject: "s", username: "jana", name: "Jana", email: "jana@bb.sk", roles: [] }
        : url.includes("/permissions/me")
          ? { project: "banskabystrica", bootstrap, grants: [] }
          : { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] };
      return Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }));
    }),
  );
  window.history.pushState({}, "", path);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const CASES: [string, string, string][] = [
  ["spaces", en.spaces.empty, en.spaces.add],
  ["endpoints", en.endpoints.empty, en.endpoints.add],
  ["datasources", en.datasources.empty, en.datasources.add],
  ["dashboards", en.dashboards.empty, en.dashboards.add],
];

describe("an empty list offers its first item", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it.each(CASES)("on %s", async (plural, empty, add) => {
    renderEmpty(`/projects/banskabystrica/${plural}`);
    const state = (await screen.findByText(empty)).closest('[role="status"]') as HTMLElement;
    expect(within(state).getByRole("button", { name: add })).toBeEnabled();
  });

  it("on approvals, as the way to the assistant", async () => {
    renderEmpty("/projects/banskabystrica/approvals");
    const state = (await screen.findByText(en.approvals.empty)).closest('[role="status"]') as HTMLElement;
    expect(within(state).getByRole("link", { name: en.approvals.emptyAction })).toBeInTheDocument();
  });

  it("disabled with the reason for a caller who may not propose", async () => {
    renderEmpty("/projects/banskabystrica/spaces", false);
    const state = (await screen.findByText(en.spaces.empty)).closest('[role="status"]') as HTMLElement;
    // The guard remounts the control once the permissions arrive, so it is queried inside the wait.
    await vi.waitFor(() =>
      expect(within(state).getByRole("button", { name: en.spaces.add })).toHaveAttribute(
        "aria-disabled",
        "true",
      ),
    );
  });
});
