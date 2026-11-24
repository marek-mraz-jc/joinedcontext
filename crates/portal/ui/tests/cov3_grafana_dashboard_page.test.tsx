// T-7589: the dashboard page's single-value variable, refresh timer, retry, refresh menu and zoom to data.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "../src/i18n";
import { DashboardPage } from "../src/pages/grafana/DashboardPage";

const BASE = "/api/v1/projects/helsinki/grafana/dashboards";
const UID = "k6";

type Answer = { status?: number; body: unknown };
let answers: Record<string, Answer>;
let asked: string[];

function view(over: Record<string, unknown> = {}) {
  return {
    uid: UID,
    orgId: 1,
    title: "Load board",
    description: null,
    from: "now-6h",
    to: "now",
    refresh: null,
    variables: [],
    panels: [{ id: 1, type: "stat", title: "Live Stats", gridPos: { x: 0, y: 0, w: 12, h: 8 } }],
    ...over,
  };
}

beforeEach(() => {
  asked = [];
  answers = {
    [`${BASE}/${UID}`]: { body: view() },
    [`${BASE}/${UID}/panels/1`]: {
      body: { frames: [{ name: "A", fields: [{ name: "Time", type: "time", values: [Date.now()] }, { name: "Value", type: "number", values: [42] }] }] },
    },
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string) => {
      const request = input instanceof Request ? input : new Request(new URL(input, "http://portal"));
      const url = new URL(request.url);
      if (url.pathname.endsWith("/panels/1")) asked.push(url.search);
      const answer = answers[url.pathname];
      if (!answer) {
        return new Response(JSON.stringify({ status: 404, title: "Not Found", detail: "not found" }), { status: 404, headers: { "Content-Type": "application/json" } });
      }
      return new Response(JSON.stringify(answer.body), { status: answer.status ?? 200, headers: { "Content-Type": "application/json" } });
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

function renderPage(search = "") {
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({ getParentRoute: () => root, path: "/", component: () => <DashboardPage project="helsinki" uid={UID} /> });
  const router = createRouter({ routeTree: root.addChildren([page]) });
  window.history.pushState({}, "", `/${search}`);
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

describe("the dashboard page, remaining paths", () => {
  it("asks the panels again with the value chosen in a single-value variable", async () => {
    answers[`${BASE}/${UID}`] = {
      body: view({
        variables: [{ name: "station", label: "Station", kind: "custom", multi: false, includeAll: true, options: [{ text: "Central", value: "central" }, { text: "North", value: "north" }], current: ["$__all"] }],
      }),
    };
    const user = userEvent.setup();
    renderPage();
    const select = await screen.findByLabelText("Station");
    await user.selectOptions(select, "north");
    await waitFor(() => expect(new URLSearchParams(asked.at(-1)).getAll("var-station")).toEqual(["north"]));
  });

  it("keeps its own range when the range cannot be read, and offers no longer range", async () => {
    answers[`${BASE}/${UID}/panels/1`] = { body: { frames: [] } };
    renderPage("?from=garbage&to=now");
    expect(await screen.findByText("No data")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Show a longer range" })).not.toBeInTheDocument();
  });

  it("asks the panel again on every refresh tick", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    renderPage("?refresh=5s");
    await waitFor(() => expect(asked.length).toBeGreaterThan(0));
    const before = asked.length;
    await act(async () => {
      vi.advanceTimersByTime(60_000);
    });
    await waitFor(() => expect(asked.length).toBeGreaterThan(before));
  });

  it("says the dashboard could not be read and reads it again on Retry", async () => {
    answers[`${BASE}/${UID}`] = { status: 500, body: { status: 500, title: "Internal Server Error", detail: "down" } };
    const user = userEvent.setup();
    renderPage();
    const retry = await screen.findByRole("button", { name: "Retry" });
    answers[`${BASE}/${UID}`] = { body: view() };
    await user.click(retry);
    expect(await screen.findByRole("heading", { level: 1, name: "Load board" })).toBeInTheDocument();
  });

  it("writes the refresh interval chosen in the menu into the address", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("heading", { level: 1, name: "Load board" });
    await user.click(screen.getByText("Off", { selector: "summary *, summary" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "30s" }));
    await waitFor(() => expect(window.location.search).toContain("refresh=30s"));
  });

  it("zooms to the data when the points lie outside the range", async () => {
    answers[`${BASE}/${UID}`] = { body: view({ panels: [{ id: 1, type: "timeseries", title: "Old series", gridPos: { x: 0, y: 0, w: 12, h: 8 } }] }) };
    answers[`${BASE}/${UID}/panels/1`] = {
      body: { frames: [{ name: "A", fields: [{ name: "Time", type: "time", values: [1000, 2000] }, { name: "Value", type: "number", values: [1, 2] }] }] },
    };
    const user = userEvent.setup();
    renderPage();
    await user.click(await screen.findByRole("button", { name: "Zoom to data" }));
    await waitFor(() => expect(window.location.search).toContain("from=%221000%22"));
    expect(window.location.search).toContain("to=%222000%22");
  });
});
