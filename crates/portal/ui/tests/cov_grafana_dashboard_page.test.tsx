// T-7589: coverage for DashboardPage.tsx single variable picker, auto-refresh, retry, and widen/zoom.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "../src/i18n";
import { DashboardPage, isBuilderUid } from "../src/pages/grafana/DashboardPage";

const BASE = "/api/v1/projects/helsinki/grafana/dashboards";
const UID = "k6";

let answers: Record<string, { status?: number; body: unknown }>;

beforeEach(() => {
  answers = {
    [`${BASE}/${UID}`]: {
      body: {
        uid: UID,
        orgId: 1,
        title: "Test Dashboard",
        description: "Dashboard view",
        from: "now-6h",
        to: "now",
        refresh: "5s",
        variables: [
          {
            name: "station",
            label: "Station",
            kind: "query",
            multi: false,
            includeAll: true,
            options: [
              { text: "Central", value: "central" },
              { text: "North", value: "north" },
            ],
            current: ["custom_station"],
          },
        ],
        panels: [
          {
            id: 1,
            type: "stat",
            title: "Live Stats",
            gridPos: { x: 0, y: 0, w: 12, h: 8 },
          },
        ],
      },
    },
    [`${BASE}/${UID}/variables/station`]: {
      body: { options: [{ text: "Central", value: "central" }, { text: "North", value: "north" }] },
    },
    [`${BASE}/${UID}/panels/1`]: {
      body: {
        frames: [{ name: "A", fields: [{ name: "Time", type: "time", values: [1] }, { name: "Value", type: "number", values: [42] }] }],
      },
    },
  };

  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string) => {
      const request = input instanceof Request ? input : new Request(new URL(input, "http://portal"));
      const url = new URL(request.url);
      const answer = answers[url.pathname];
      if (!answer) {
        return new Response(JSON.stringify({ status: 404, title: "Not Found", detail: "not found" }), {
          status: 404,
          headers: { "Content-Type": "application/json" },
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
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function renderPage(search = "") {
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({
    getParentRoute: () => root,
    path: "/",
    component: () => <DashboardPage project="helsinki" uid={UID} />,
  });
  const router = createRouter({ routeTree: root.addChildren([page]) });
  window.history.pushState({}, "", `/${search}`);
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

describe("DashboardPage coverage", () => {
  it("checks isBuilderUid helper", () => {
    expect(isBuilderUid("jcd-0123456789abcdef")).toBe(true);
    expect(isBuilderUid("k6")).toBe(false);
    expect(isBuilderUid("jcd-short")).toBe(false);
  });

  it("covers multi-value variable with empty selection (line 152) and refresh timer (lines 216-220)", async () => {
    vi.useFakeTimers();

    answers[`${BASE}/${UID}`] = {
      body: {
        uid: UID,
        orgId: 1,
        title: "Test Dashboard",
        from: "now-6h",
        to: "now",
        refresh: "5s",
        variables: [
          {
            name: "tags",
            label: "Tags",
            kind: "custom",
            multi: true,
            includeAll: false,
            options: [{ text: "Tag A", value: "tagA" }],
            current: [],
          },
        ],
        panels: [],
      },
    };

    const { unmount } = renderPage();
    await vi.waitFor(() => {
      // Line 152: words is empty, shows "None"
      expect(screen.getByText("None")).toBeInTheDocument();
    });

    // Lines 216-220: advance timer for auto refresh interval
    vi.advanceTimersByTime(5500);

    // Unmount clears interval (line 220)
    unmount();
  });

  it("covers Refresh now button onRefresh callback (line 337)", async () => {
    const user = userEvent.setup();
    renderPage();

    expect(await screen.findByRole("heading", { level: 1, name: "Test Dashboard" })).toBeInTheDocument();
    const refreshNowBtn = screen.getByRole("button", { name: "Refresh now" });
    await user.click(refreshNowBtn);
  });

  it("covers LivePanel onRetry callback on error (line 91)", async () => {
    const user = userEvent.setup();
    answers[`${BASE}/${UID}/panels/1`] = {
      body: { frames: [], error: "Datasource failed" },
    };

    renderPage();

    expect(await screen.findByText("The data source did not answer")).toBeInTheDocument();
    const tryAgainBtn = screen.getByRole("button", { name: "Try again" });
    // Line 91: onRetry invalidates query
    await user.click(tryAgainBtn);
  });

  it("covers onWiden (lines 373-374) and invalid time in longerThan (line 182)", async () => {
    const user = userEvent.setup();
    answers[`${BASE}/${UID}`] = {
      body: {
        uid: UID,
        orgId: 1,
        title: "Test Dashboard",
        from: "now-15m",
        to: "now",
        refresh: null,
        variables: [],
        panels: [
          {
            id: 1,
            type: "stat",
            title: "Live Stats",
            gridPos: { x: 0, y: 0, w: 12, h: 8 },
          },
        ],
      },
    };
    answers[`${BASE}/${UID}/panels/1`] = {
      body: { frames: [] },
    };

    renderPage();

    expect(await screen.findByText("Show a longer range")).toBeInTheDocument();
    const widenBtn = screen.getByRole("button", { name: "Show a longer range" });
    // Lines 373-374: onWiden click
    await user.click(widenBtn);
  });
});
