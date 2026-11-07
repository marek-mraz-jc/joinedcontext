/**
 * T-4221 (photo audit T-4163, UI-01): the Policies list held about 60 rows and no way to narrow
 * them; the only search box on the page, "Try a person", does not filter the list. A filter over
 * the name, the title, the space and the grantee narrows it and says how many of how many show.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";

const policy = (name: string, space: string, group: string, title?: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Policy",
  metadata: { name, namespace: "helsinki", ...(title ? { title: { en: title } } : {}) },
  spec: { contextSpaceRef: space, assigner: "did:web:hel.fi", assignee: { kind: "group", id: group }, operations: ["retrieveOps"] },
  status: { phase: "Live" },
});

const POLICIES = [
  policy("air-team-reads", "ovzdusie", "air-team"),
  policy("app-bike-weather-demand-read", "mobility", "app-bike-weather-demand", "Bike demand reads"),
  policy("app-kpi-forecast-read", "kpi", "app-kpi-forecast"),
];

beforeEach(async () => {
  await i18n.changeLanguage("en");
  window.history.pushState({}, "", "/projects/helsinki/policies");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL((input as Request).url);
      const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "demo.steward", roles: ["portal-admin"] });
      if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
      if (url.pathname === "/api/v1/projects/helsinki/policies") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: POLICIES });
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
});

const rows = () => within(screen.getByRole("table")).getAllByRole("row").slice(1);

describe("the Policies list narrows to what a person types", () => {
  it("filters by title, space and grantee and counts what shows", async () => {
    const user = userEvent.setup();
    await screen.findByRole("link", { name: "air-team-reads" });
    expect(rows()).toHaveLength(3);
    const box = screen.getByRole("searchbox", { name: i18n.t("policies.filter.label") });

    await user.type(box, "bike demand");
    expect(rows()).toHaveLength(1);
    expect(screen.getAllByRole("status").some((status) => status.textContent === "1 of 3 policies")).toBe(true);

    await user.clear(box);
    await user.type(box, "OVZDUSIE");
    expect(rows()).toHaveLength(1);
    expect(within(rows()[0]).getByRole("link", { name: "air-team-reads" })).toBeInTheDocument();

    await user.clear(box);
    await user.type(box, "app-kpi");
    expect(rows()).toHaveLength(1);
  });

  it("says nothing matches instead of showing an empty table or the first-run state", async () => {
    const user = userEvent.setup();
    await screen.findByRole("link", { name: "air-team-reads" });
    await user.type(screen.getByRole("searchbox", { name: i18n.t("policies.filter.label") }), "zzz");
    expect(screen.getByText(i18n.t("policies.filter.none", { query: "zzz" }))).toBeInTheDocument();
    expect(screen.queryByText(i18n.t("policies.empty"))).toBeNull();
  });
});
