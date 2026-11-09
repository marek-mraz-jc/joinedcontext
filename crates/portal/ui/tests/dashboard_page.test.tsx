/**
 * The dashboard page around its panels: it reads the dashboard, offers its range, refresh and
 * variables (a query variable's values from the Portal), asks each panel with them, and says why
 * when the dashboard cannot be read.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import "../src/i18n";
import { DashboardPage } from "../src/pages/grafana/DashboardPage";
import { ProjectDashboards } from "../src/pages/grafana/GrafanaPage";

const BASE = "/api/v1/projects/helsinki/grafana/dashboards";
const VIEW = {
  uid: "k6",
  orgId: 4,
  title: "k6 load",
  description: null,
  from: "now-6h",
  to: "now",
  refresh: null,
  variables: [{ name: "testid", label: "Test run", kind: "query", multi: true, includeAll: true, options: [], current: ["$__all"] }],
  panels: [{ id: 3, type: "stat", title: "Checks passed", gridPos: { x: 0, y: 0, w: 6, h: 4 }, fieldConfig: { defaults: { unit: "percent" }, overrides: [] } }],
};

let answers: Record<string, { status?: number; body: unknown }>;
let asked: string[];

beforeEach(() => {
  asked = [];
  answers = {
    [BASE]: { body: { ready: true, items: [{ uid: "k6", title: "k6 load", tags: ["load"] }] } },
    [`${BASE}/k6`]: { body: VIEW },
    [`${BASE}/k6/variables/testid`]: { body: { options: [{ text: "run-41", value: "run-41" }, { text: "run-42", value: "run-42" }] } },
    [`${BASE}/k6/panels/3`]: { body: { frames: [{ name: "", refId: "A", fields: [{ name: "Time", type: "time", values: [1, 2] }, { name: "Value", type: "number", values: [97.5, 99.1] }] }] } },
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string) => {
      const request = input instanceof Request ? input : new Request(new URL(input, "http://portal"));
      const url = new URL(request.url);
      if (url.pathname.endsWith("/panels/3")) asked.push(url.search);
      const answer = answers[url.pathname];
      if (!answer) return new Response(JSON.stringify({ status: 404, title: "Not Found", detail: "no such dashboard" }), { status: 404, headers: { "Content-Type": "application/json" } });
      return new Response(JSON.stringify(answer.body), { status: answer.status ?? 200, headers: { "Content-Type": "application/json" } });
    }),
  );
});

afterEach(() => vi.unstubAllGlobals());

function renderAt(node: () => React.JSX.Element) {
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({ getParentRoute: () => root, path: "/", component: node });
  const target = createRoute({ getParentRoute: () => root, path: "/projects/$project/grafana/dashboards/$uid", component: () => <p>the dashboard</p> });
  const list = createRoute({ getParentRoute: () => root, path: "/projects/$project/grafana", component: () => <p>the list</p> });
  const router = createRouter({ routeTree: root.addChildren([page, target, list]) });
  window.history.pushState({}, "", "/");
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

describe("the dashboard page", () => {
  it("draws its panels with the variable's current value and offers the values the Portal read", async () => {
    renderAt(() => <DashboardPage project="helsinki" uid="k6" />);
    expect(await screen.findByRole("heading", { level: 1, name: "k6 load" })).toBeInTheDocument();
    expect(await screen.findByText("99.1%")).toBeInTheDocument();
    // Grafana asks a datasource with the range as epochs: six hours back from now.
    const first = new URLSearchParams(asked[0]);
    expect([...first.keys()]).toEqual(["from", "to", "maxDataPoints", "var-testid"]);
    expect(Number(first.get("to")) - Number(first.get("from"))).toBe(6 * 3_600_000);
    expect(first.getAll("var-testid")).toEqual(["$__all"]);
    const user = userEvent.setup();
    await user.click(screen.getByText("All", { selector: "summary" }));
    const picker = screen.getByRole("group", { name: "Test run" });
    await waitFor(() => expect(picker).toHaveTextContent("run-42"));
    expect([...picker.querySelectorAll("label")].map((label) => label.textContent)).toEqual(["All", "run-41", "run-42"]);
    await user.click(screen.getByRole("checkbox", { name: "run-41" }));
    await user.click(screen.getByRole("checkbox", { name: "run-42" }));
    // A multi-value variable sends one var- per value, as Grafana's frontend does.
    await waitFor(() => expect(new URLSearchParams(asked.at(-1)).getAll("var-testid")).toEqual(["run-41", "run-42"]));
  });

  it("asks again over a new range and on Refresh now", async () => {
    const user = userEvent.setup();
    renderAt(() => <DashboardPage project="helsinki" uid="k6" />);
    await screen.findByText("99.1%");
    await user.click(screen.getAllByText("Last 6 hours")[0]);
    await user.click(screen.getByRole("button", { name: "Last 24 hours" }));
    await waitFor(() => {
      const last = new URLSearchParams(asked.at(-1));
      expect(Number(last.get("to")) - Number(last.get("from"))).toBe(24 * 3_600_000);
    });
    expect(window.location.search).toBe("?from=now-24h&to=now");
    const before = asked.length;
    await user.click(screen.getByRole("button", { name: "Refresh now" }));
    await waitFor(() => expect(asked.length).toBeGreaterThan(before));
  });

  it("says a dashboard that is not there is not in the project, with the way back", async () => {
    renderAt(() => <DashboardPage project="helsinki" uid="gone" />);
    expect(await screen.findByText("This dashboard is not in the project")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to the dashboards" })).toHaveAttribute("href", "/projects/helsinki/grafana");
    expect(screen.getByRole("heading", { level: 1 })).toBeInTheDocument();
  });

  it("says why a dashboard cannot be read when the Portal fails", async () => {
    answers[`${BASE}/k6`] = { status: 409, body: { status: 409, title: "Conflict", detail: "dashboards are not set up on this installation" } };
    renderAt(() => <DashboardPage project="helsinki" uid="k6" />);
    expect(await screen.findByText(/not set up on this installation/)).toBeInTheDocument();
  });

  it("sums up several failed panels above them and asks them all again", async () => {
    const user = userEvent.setup();
    const panels = [1, 2, 3].map((id) => ({ ...VIEW.panels[0], id, title: `Panel ${id}`, gridPos: { x: 0, y: id * 4, w: 6, h: 4 } }));
    answers[`${BASE}/k6`] = { body: { ...VIEW, variables: [], panels } };
    for (const id of [1, 2]) answers[`${BASE}/k6/panels/${id}`] = { body: { frames: [], error: "connection refused" } };
    renderAt(() => <DashboardPage project="helsinki" uid="k6" />);
    expect(await screen.findByText("2 panels could not be read")).toBeInTheDocument();
    expect(screen.getAllByText("Not read: see the notice above the panels.")).toHaveLength(2);
    expect(await screen.findByText("99.1%")).toBeInTheDocument();
    const before = asked.length;
    await user.click(screen.getByRole("button", { name: "Try all again" }));
    await waitFor(() => expect(asked.length).toBeGreaterThan(before));
  });

  it("opens the tables page in the organization that holds the dashboard, over the asked range", async () => {
    renderAt(() => <DashboardPage project="helsinki" uid="k6" />);
    const link = await screen.findByRole("link", { name: /Open as tables/ });
    const href = new URL(link.getAttribute("href") ?? "", "http://portal");
    expect(href.pathname).toBe("/dashboards/d/k6");
    expect(Object.fromEntries(href.searchParams)).toEqual({ orgId: "4", from: "now-6h", to: "now" });
  });

  it("says a dashboard without panels is empty", async () => {
    answers[`${BASE}/k6`] = { body: { ...VIEW, variables: [], panels: [] } };
    renderAt(() => <DashboardPage project="helsinki" uid="k6" />);
    expect(await screen.findByText("This dashboard has no panels")).toBeInTheDocument();
  });
});

describe("the project's dashboards on the Grafana page", () => {
  it("links each dashboard to its page", async () => {
    renderAt(() => <ProjectDashboards project="helsinki" />);
    expect(await screen.findByRole("link", { name: "k6 load" })).toHaveAttribute("href", "/projects/helsinki/grafana/dashboards/k6");
    expect(screen.getByRole("list", { name: "Dashboards by folder" })).toHaveTextContent("load");
  });

  it("browses folders, searches, filters by tag and sorts, as Grafana's list does", async () => {
    const user = userEvent.setup();
    answers[BASE] = {
      body: {
        ready: true,
        items: [
          { uid: "air", title: "Air quality", tags: ["city"], folder: "Environment" },
          { uid: "noise", title: "Noise", tags: [], folder: "Environment" },
          { uid: "k6", title: "k6 load", tags: ["load"] },
          { uid: "bus", title: "Buses", tags: ["city"], folder: "Transport" },
        ],
      },
    };
    renderAt(() => <ProjectDashboards project="helsinki" />);
    const tree = await screen.findByRole("list", { name: "Dashboards by folder" });
    // Folders first, closed, then General's dashboards.
    const folders = screen.getAllByRole("button", { expanded: false });
    expect(folders.map((button) => button.textContent)).toEqual(["Environment2 dashboards", "Transport1 dashboard"]);
    expect(tree).toHaveTextContent("k6 load");
    expect(screen.queryByRole("link", { name: "Air quality" })).toBeNull();
    await user.click(folders[0]);
    expect(screen.getByRole("link", { name: "Air quality" })).toBeInTheDocument();

    await user.type(screen.getByLabelText("Search dashboards"), "BU");
    const results = screen.getByRole("list", { name: "Search results" });
    expect(results).toHaveTextContent("Buses");
    expect(results).toHaveTextContent("Transport");
    expect(screen.queryByRole("link", { name: "Noise" })).toBeNull();
    await user.clear(screen.getByLabelText("Search dashboards"));

    await user.selectOptions(screen.getByLabelText("Tag"), "city");
    await user.selectOptions(screen.getByLabelText("Sort"), "alpha-desc");
    const links = screen.getAllByRole("link").map((link) => link.textContent);
    expect(links).toEqual(["Buses", "Air quality"]);

    await user.type(screen.getByLabelText("Search dashboards"), "nothing like it");
    expect(screen.getByRole("status")).toHaveTextContent("No dashboard matches this search.");
  });

  it("says why there are none", async () => {
    answers[BASE] = { body: { ready: false, reason: "the dashboards service has no organization for helsinki yet", items: [] } };
    renderAt(() => <ProjectDashboards project="helsinki" />);
    expect(await screen.findByText(/no organization for helsinki/)).toBeInTheDocument();
  });
});
