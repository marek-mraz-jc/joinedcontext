/**
 * T-7161: the saved charts move into one Saved charts dashboard. The check runs as a dry run and
 * says what the step would do; the step it names is then proposed, or, once the dashboard is
 * merged, the charts it holds are removed. Every chart it does not hold is named with the reason.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { SavedChartsMove } from "../src/pages/grafana/SavedChartsMove";
import "../src/i18n";

const MOVE = "/api/v1/projects/helsinki/grafana/builder/saved-charts";
const UID = "jcd-0123456789abcdef";
const KEPT = [{ id: "c", title: "Stations", reason: "a map chart: the dashboard builder has no map panel yet" }];
let answer: (dryRun: boolean) => { status: number; body: unknown };
let sent: unknown[];

beforeEach(() => {
  sent = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string) => {
      const request = input instanceof Request ? input : new Request(new URL(input, "http://portal"));
      if (new URL(request.url).pathname !== MOVE || request.method !== "POST") return new Response("{}", { status: 404 });
      const body = (await request.clone().json()) as { dryRun: boolean };
      sent.push(body);
      const { status, body: out } = answer(body.dryRun);
      return new Response(JSON.stringify(out), { status, headers: { "Content-Type": "application/json" } });
    }),
  );
});

afterEach(() => vi.unstubAllGlobals());

function renderMove() {
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({ getParentRoute: () => root, path: "/", component: () => <SavedChartsMove project="helsinki" /> });
  const router = createRouter({ routeTree: root.addChildren([page]) });
  window.history.pushState({}, "", "/");
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

describe("moving the saved charts into a dashboard", () => {
  it("checks first, names what stays, then proposes the dashboard and links its change", async () => {
    answer = (dryRun) =>
      dryRun
        ? { status: 200, body: { uid: UID, stage: "propose", dryRun, before: 3, panels: 2, moved: ["a", "b"], kept: KEPT } }
        : { status: 202, body: { uid: UID, stage: "propose", dryRun, before: 3, panels: 2, moved: ["a", "b"], kept: KEPT, change: { metadata: { name: "pr-12" } } } };
    renderMove();
    const user = userEvent.setup();
    expect(screen.queryByRole("button", { name: "Propose the Saved charts dashboard" })).not.toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "Check the move" }));
    expect(await screen.findByText(/3 saved charts: the Saved charts dashboard gets 2 panels/)).toBeInTheDocument();
    expect(screen.getByText(/Stations: a map chart/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Propose the Saved charts dashboard" }));
    expect(await screen.findByRole("link", { name: "Open the change" })).toHaveAttribute("href", "/projects/helsinki/approvals/pr-12");
    expect(sent).toEqual([{ dryRun: true }, { dryRun: false }]);
  });

  it("after the merge removes only what the dashboard holds and says how many", async () => {
    answer = (dryRun) => ({ status: 200, body: { uid: UID, stage: "remove", dryRun, before: 3, panels: 2, moved: ["a"], kept: KEPT } });
    renderMove();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Check the move" }));
    expect(await screen.findByText(/holds 1 of these charts/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Remove 1 moved chart" }));
    expect(await screen.findByText("Removed 1 moved chart")).toBeInTheDocument();
  });

  it("says why the server refused", async () => {
    answer = () => ({ status: 409, body: { status: 409, title: "Conflict", detail: "a change of this dashboard is open already: pr-12; approve or reject it first" } });
    renderMove();
    await userEvent.setup().click(await screen.findByRole("button", { name: "Check the move" }));
    expect(await screen.findByText(/a change of this dashboard is open already/)).toBeInTheDocument();
  });
});
