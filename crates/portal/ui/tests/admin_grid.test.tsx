// covers (T-2137, the module gate in gate_modules.test.ts): src/components/grid/AdminGrid.tsx.
/**
 * T-6739: an admin list on the shared data grid, walked through the real Approvals page. The
 * rows are what `/changes` returned for this caller; the grid sorts, filters, searches and pages
 * them, and a failed read is the Portal's failure panel with a retry.
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { AdminGrid } from "../src/components/grid/AdminGrid";

const IDENTITY = { subject: "s", username: "jana", email: "jana@example.org", roles: ["portal-approver"] };

function change(name: string, endpoint: string, createdAt: string, lane: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ChangeProposal",
    metadata: { name, namespace: "city" },
    summary: { key: "change.summary.update", params: { kind: "Endpoint", name: endpoint, fields: 1 } },
    author: { name: "Marek Mráz", email: "marek@example.org" },
    createdAt,
    status: { lane, phase: "PendingApproval", plan: { update: 1 } },
  };
}

const ITEMS = [
  change("chg-b", "buses", "2026-03-02T12:00:00Z", "green"),
  change("chg-a", "air-quality", "2026-03-04T12:00:00Z", "red"),
  change("chg-c", "cycling", "2026-03-03T12:00:00Z", "yellow"),
];

function renderAt(answer: () => Response) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
    if (url.includes("/auth/me")) return Promise.resolve(json(IDENTITY));
    if (url.endsWith("/api/v1/projects/city/changes")) return Promise.resolve(answer());
    return Promise.resolve(json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }));
  });
  vi.stubGlobal("fetch", fetchMock);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

const ok = (items: unknown[]) => () =>
  new Response(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items }), {
    headers: { "Content-Type": "application/json" },
  });
const summaries = () =>
  [...document.querySelectorAll(".jc-data-grid tbody tr")].map((tr) => tr.querySelector("a")?.textContent ?? "");

describe("an admin list on the shared grid (Approvals)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/city/approvals");
    try {
      window.localStorage.clear();
    } catch {
      // no storage in this run
    }
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("sorts from the header menu, searches and filters the rows the API returned", async () => {
    renderAt(ok(ITEMS));
    await screen.findByRole("link", { name: /air-quality/ });
    expect(summaries()).toHaveLength(3);

    fireEvent.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: ${en.approvals.created}` }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: en.entityGrid.sortAscDate }));
    expect(summaries().map((text) => text.match(/"([^"]+)"/)?.[1])).toEqual(["buses", "cycling", "air-quality"]);

    fireEvent.change(screen.getByRole("searchbox", { name: en.dataGrid.searchRows }), { target: { value: "cycl" } });
    expect(summaries()).toHaveLength(1);
    fireEvent.change(screen.getByRole("searchbox", { name: en.dataGrid.searchRows }), { target: { value: "nothing" } });
    expect(screen.getByText(en.dataGrid.noMatch)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: en.entityGrid.clearFilters }));
    expect(summaries()).toHaveLength(3);
  });

  it("hides a column from Arrange columns and keeps the row name always shown", async () => {
    renderAt(ok(ITEMS));
    await screen.findByRole("link", { name: /air-quality/ });
    fireEvent.click(screen.getByRole("button", { name: en.entityGrid.arrangeColumns }));
    const panel = screen.getByRole("dialog", { name: en.entityGrid.arrangeColumns });
    expect(within(panel).getByRole("checkbox", { name: `${en.entityGrid.showColumn}: ${en.approvals.summary}` })).toBeDisabled();
    fireEvent.click(within(panel).getByRole("checkbox", { name: `${en.entityGrid.showColumn}: ${en.approvals.author}` }));
    expect(screen.queryByRole("columnheader", { name: new RegExp(en.approvals.author) })).toBeNull();
  });

  it("says what to do next when nothing is proposed", async () => {
    renderAt(ok([]));
    expect(await screen.findByText(en.approvals.empty)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.approvals.emptyAction })).toBeInTheDocument();
  });

  it("shows the API's reason with a retry when the read fails", async () => {
    renderAt(
      () =>
        new Response(JSON.stringify({ type: "about:blank", title: "Unavailable", status: 503, detail: "The broker did not answer." }), {
          status: 503,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );
    expect(await screen.findByText(/The broker did not answer/)).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("draws a skeleton while the list loads, in the Portal's words", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <AdminGrid<{ name: string }>
          query={{ isPending: true, isError: false, error: null, refetch: () => undefined }}
          caption="Policies"
          storageKey="test"
          rows={[]}
          rowKey={(row) => row.name}
          columns={[{ key: "name", label: "Name", text: (row) => row.name }]}
          empty={<p>none</p>}
        />
      </I18nextProvider>,
    );
    expect(screen.getByRole("table", { name: "Policies" })).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("status")).toHaveTextContent(en.entityGrid.loading);
  });
});
