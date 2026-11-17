/**
 * T-7150: a builder dashboard's history on its page. The versions newest first with who and
 * when, an older version compared with the current one as the choices that differ, and a restore
 * asked twice that proposes a Change; a reader compares but cannot restore, and a version the
 * server refuses says why instead of a diff.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import "../src/i18n";
import { boardChanges, DashboardHistory } from "../src/pages/grafana/DashboardHistory";

const UID = "jcd-0123456789abcdef";
const BASE = `/api/v1/projects/helsinki/grafana/builder/${UID}/versions`;
const OLD = "1111111111111111111111111111111111111111";
const NEW = "2222222222222222222222222222222222222222";
const board = (title: string, attribute: string) => ({
  title,
  panels: [{ id: 1, kind: "stat", title: "Free bikes", endpoint: "bikes-open", attribute }],
});

let answers: Record<string, { status?: number; body: unknown }>;
let posted: string[];

beforeEach(() => {
  posted = [];
  answers = {
    [BASE]: {
      body: {
        items: [
          { sha: NEW, author: "Eva Nováková", date: "2026-11-16T10:00:00Z", message: "change dashboard Bike stations" },
          { sha: OLD, author: "Ada", date: "2026-11-15T09:00:00Z", message: "add dashboard Bike stations" },
        ],
      },
    },
    [`${BASE}/${NEW}`]: { body: { sha: NEW, board: board("Bike stations, renamed", "availableBikeNumber") } },
    [`${BASE}/${OLD}`]: { body: { sha: OLD, board: board("Bike stations", "status") } },
    [`${BASE}/${OLD}/restore`]: { status: 202, body: { uid: UID, change: { metadata: { name: "pr-12" } } } },
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(input, "http://portal"), init);
      const url = new URL(request.url);
      if (request.method === "POST") posted.push(url.pathname);
      const answer = answers[url.pathname];
      if (!answer) return new Response(JSON.stringify({ status: 404, title: "Not Found", detail: "no such version" }), { status: 404, headers: { "Content-Type": "application/problem+json" } });
      return new Response(JSON.stringify(answer.body), { status: answer.status ?? 200, headers: { "Content-Type": answer.status && answer.status >= 400 ? "application/problem+json" : "application/json" } });
    }),
  );
});

afterEach(() => vi.unstubAllGlobals());

function renderHistory(mayEdit: boolean) {
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({ getParentRoute: () => root, path: "/", component: () => <DashboardHistory project="helsinki" uid={UID} mayEdit={mayEdit} /> });
  const change = createRoute({ getParentRoute: () => root, path: "/projects/$project/approvals/$id", component: () => <p>the change</p> });
  const router = createRouter({ routeTree: root.addChildren([page, change]) });
  window.history.pushState({}, "", "/");
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

describe("a dashboard's history (T-7150)", () => {
  it("lists the versions newest first with who and when", async () => {
    renderHistory(true);
    const items = await screen.findAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent("change dashboard Bike stations");
    expect(items[0]).toHaveTextContent("(current)");
    expect(items[0]).toHaveTextContent("Eva Nováková");
    expect(items[1]).toHaveTextContent("Ada");
    expect(items[1]).toHaveTextContent("1111111");
  });

  it("compares an older version with the current one as the choices that differ", async () => {
    renderHistory(false);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Compare with current" }));
    expect(await screen.findByText("Bike stations, renamed")).toBeInTheDocument();
    expect(screen.getByText("status")).toBeInTheDocument();
    expect(screen.getByText("availableBikeNumber")).toBeInTheDocument();
    expect(screen.queryByText("bikes-open")).not.toBeInTheDocument();
    // The compared version stands out from the others, and its message wraps rather than cut at 400 px.
    const [newest, compared] = screen.getAllByRole("listitem");
    expect(compared).toHaveClass("bg-surface-subtle");
    expect(newest).not.toHaveClass("bg-surface-subtle");
    expect(compared.querySelector(".truncate")).toBeNull();
  });

  it("restores only after asking twice, and shows the Change it opened", async () => {
    renderHistory(true);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Restore" }));
    expect(posted).toEqual([]);
    expect(screen.getByText("Restore this version?")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Yes, propose the restore" }));
    await waitFor(() => expect(posted).toEqual([`${BASE}/${OLD}/restore`]));
    expect(await screen.findByRole("link", { name: "Open the change" })).toHaveAttribute("href", "/projects/helsinki/approvals/pr-12");
  });

  it("offers a reader no restore and says what it needs", async () => {
    renderHistory(false);
    expect(await screen.findByRole("button", { name: "Restore" })).toBeDisabled();
    expect(screen.getAllByText(/needs the right to build dashboards/).length).toBeGreaterThan(0);
  });

  it("says why a version cannot be compared", async () => {
    answers[`${BASE}/${OLD}`] = { status: 403, body: { status: 403, title: "Forbidden", detail: "this version reads an Endpoint you may not read; ask a project owner for read access" } };
    renderHistory(false);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Compare with current" }));
    expect(await screen.findByText("These versions cannot be compared")).toBeInTheDocument();
    expect(screen.getByText(/reads an Endpoint you may not read/)).toBeInTheDocument();
  });

  it("names an empty history and what comes next", async () => {
    answers[BASE] = { body: { items: [] } };
    renderHistory(true);
    expect(await screen.findByText("No versions yet")).toBeInTheDocument();
  });
});

describe("boardChanges", () => {
  it("lists only the leaves that differ, added and removed included", () => {
    const changes = boardChanges({ title: "A", panels: [{ id: 1 }] }, { title: "B", panels: [{ id: 1 }, { id: 2 }] });
    expect(changes).toEqual([
      { path: "panels[1].id", to: 2 },
      { path: "title", from: "A", to: "B" },
    ]);
  });
});
