// T-7589: coverage for DashboardHistory.tsx cancel button and edge cases.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "../src/i18n";
import { DashboardHistory } from "../src/pages/grafana/DashboardHistory";

const UID = "jcd-0123456789abcdef";
const BASE = `/api/v1/projects/helsinki/grafana/builder/${UID}/versions`;
const OLD = "1111111111111111111111111111111111111111";
const NEW = "2222222222222222222222222222222222222222";

let answers: Record<string, { status?: number; body: unknown }>;

beforeEach(() => {
  answers = {
    [BASE]: {
      body: {
        items: [
          { sha: NEW, author: "Eva", date: "2026-11-16T10:00:00Z", message: "new version" },
          { sha: OLD, author: "Ada", date: "invalid-date", message: "" },
        ],
      },
    },
    [`${BASE}/${NEW}`]: { body: { sha: NEW, board: { title: "New Board", panels: [] } } },
    [`${BASE}/${OLD}`]: { body: { sha: OLD, board: { title: "Old Board", panels: [] } } },
  };

  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(input, "http://portal"), init);
      const url = new URL(request.url);
      const answer = answers[url.pathname];
      if (!answer) {
        return new Response(JSON.stringify({ status: 404, title: "Not Found", detail: "not found" }), {
          status: 404,
          headers: { "Content-Type": "application/problem+json" },
        });
      }
      return new Response(JSON.stringify(answer.body), {
        status: answer.status ?? 200,
        headers: { "Content-Type": "application/json" },
      });
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function renderHistory(mayEdit = true) {
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <DashboardHistory project="helsinki" uid={UID} mayEdit={mayEdit} />,
  });
  const router = createRouter({ routeTree: root.addChildren([page]) });
  window.history.pushState({}, "", "/");
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

describe("DashboardHistory coverage", () => {
  it("cancels restore confirmation on cancel button click (line 177)", async () => {
    const user = userEvent.setup();
    renderHistory(true);

    const restoreBtn = await screen.findByRole("button", { name: "Restore" });
    await user.click(restoreBtn);

    expect(screen.getByText("Restore this version?")).toBeInTheDocument();

    // Line 177: Cancel button
    const cancelBtn = screen.getByRole("button", { name: "Cancel" });
    await user.click(cancelBtn);

    expect(screen.queryByText("Restore this version?")).toBeNull();
  });

  it("handles restore mutation error display", async () => {
    answers[`${BASE}/${OLD}/restore`] = {
      status: 500,
      body: { title: "Server Error", status: 500, detail: "restore failed on server" },
    };

    const user = userEvent.setup();
    renderHistory(true);

    const restoreBtn = await screen.findByRole("button", { name: "Restore" });
    await user.click(restoreBtn);

    const confirmBtn = screen.getByRole("button", { name: "Yes, propose the restore" });
    await user.click(confirmBtn);

    expect(await screen.findByText("restore failed on server")).toBeInTheDocument();
  });
});
