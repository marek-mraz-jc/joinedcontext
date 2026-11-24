// T-7589: the Grafana page's failed read, its form without options or endpoints, and the dashboards waiting for approval.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "../src/i18n";
import { GrafanaPage, ProjectDashboards, VisualizationForm } from "../src/pages/grafana/GrafanaPage";

type Answer = { status?: number; body: unknown };
let answers: Record<string, Answer>;

const problem = (status: number, detail: string) => ({ status, body: { status, title: "Refused", detail } });

beforeEach(() => {
  answers = {};
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string) => {
      const request = input instanceof Request ? input : new Request(new URL(input, "http://portal"));
      const url = new URL(request.url);
      const answer = answers[url.pathname];
      if (!answer) return new Response("{}", { status: 404, headers: { "Content-Type": "application/json" } });
      return new Response(JSON.stringify(answer.body), { status: answer.status ?? 200, headers: { "Content-Type": "application/json" } });
    }),
  );
});

afterEach(() => vi.unstubAllGlobals());

function client() {
  return new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
}

function renderRouted(node: () => React.JSX.Element) {
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({ getParentRoute: () => root, path: "/", component: node });
  const edit = createRoute({ getParentRoute: () => root, path: "/projects/$project/grafana/dashboards/$uid/edit", component: () => <p>editor</p> });
  const change = createRoute({ getParentRoute: () => root, path: "/projects/$project/approvals/$id", component: () => <p>change</p> });
  const created = createRoute({ getParentRoute: () => root, path: "/projects/$project/grafana/new", component: () => <p>new</p> });
  const router = createRouter({ routeTree: root.addChildren([page, edit, change, created]) });
  window.history.pushState({}, "", "/");
  return render(
    <QueryClientProvider client={client()}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

describe("the Grafana page, remaining paths", () => {
  it("says why the page could not be read", async () => {
    answers["/api/v1/projects/helsinki/grafana"] = problem(500, "grafana down");
    answers["/api/v1/projects/helsinki/grafana/dashboards"] = { body: { ready: true, items: [] } };
    answers["/api/v1/projects/helsinki/grafana/builder"] = { body: { mayEdit: false, pending: [], refreshes: [], units: [], colors: [] } };
    renderRouted(() => <GrafanaPage project="helsinki" />);
    expect(await screen.findByRole("heading", { level: 1, name: "Dashboards" })).toBeInTheDocument();
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.queryByLabelText("Endpoint")).not.toBeInTheDocument();
  });

  it("says why the form's choices could not be read", async () => {
    answers["/api/v1/projects/helsinki/grafana/options"] = problem(403, "no read access");
    renderRouted(() => <VisualizationForm project="helsinki" />);
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.queryByLabelText("Endpoint")).not.toBeInTheDocument();
  });

  it("tells what to do first when no endpoint is readable", async () => {
    answers["/api/v1/projects/helsinki/grafana/options"] = {
      body: { endpoints: [{ name: "ops", title: "Bike operations", slug: "opsslug", public: false, usable: false }], types: [], attributes: [] },
    };
    renderRouted(() => <VisualizationForm project="helsinki" />);
    expect(await screen.findByText("No Endpoint to draw from")).toBeInTheDocument();
    expect(screen.getByText(/You may read no Endpoint of this project yet/)).toBeInTheDocument();
  });

  it("lists the dashboards waiting for approval, each opening its edit and its change", async () => {
    answers["/api/v1/projects/helsinki/grafana/dashboards"] = { body: { ready: true, items: [] } };
    answers["/api/v1/projects/helsinki/grafana/builder"] = {
      body: { mayEdit: true, pending: [{ uid: "jcd-0123456789abcdef", title: "Air board", change: "chg-77", url: "https://git.example/mr/1" }], refreshes: [], units: [], colors: [] },
    };
    renderRouted(() => <ProjectDashboards project="helsinki" />);
    expect(await screen.findByRole("heading", { level: 3, name: "Waiting for approval" })).toBeInTheDocument();
    expect(screen.getByText("Air board")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open" })).toHaveAttribute("href", "/projects/helsinki/grafana/dashboards/jcd-0123456789abcdef/edit");
    expect(screen.getByRole("link", { name: "Open the change" })).toHaveAttribute("href", "/projects/helsinki/approvals/chg-77");
  });
});
