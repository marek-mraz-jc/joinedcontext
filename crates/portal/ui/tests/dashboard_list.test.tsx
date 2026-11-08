/**
 * The project's Grafana dashboard list, as Grafana's browse page lists them: folders in title
 * order with General's dashboards after them, a search over titles, one tag, A–Z or Z–A, and an
 * empty search that offers to clear itself.
 */
import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import "../src/i18n";
import { browse, DashboardList, folders } from "../src/pages/grafana/DashboardList";

const ITEMS = [
  { uid: "k6", title: "k6 load", tags: ["load", "platform"], folder: "Platform" },
  { uid: "air", title: "Helsinki air", tags: ["air"], folder: "Helsinki" },
  { uid: "broker", title: "Context broker", tags: ["platform"], folder: "Platform" },
  { uid: "scratch", title: "Scratch board", tags: [] },
];

function renderList() {
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({ getParentRoute: () => root, path: "/", component: () => <DashboardList project="helsinki" items={ITEMS} /> });
  const target = createRoute({ getParentRoute: () => root, path: "/projects/$project/grafana/dashboards/$uid", component: () => <p>the dashboard</p> });
  window.history.pushState({}, "", "/");
  return render(<RouterProvider router={createRouter({ routeTree: root.addChildren([page, target]) })} />);
}

describe("the dashboard list", () => {
  it("searches titles case-blind, keeps one tag and sorts either way", () => {
    expect(browse(ITEMS, "HELSINKI", "", "alpha-asc").map((item) => item.uid)).toEqual(["air"]);
    expect(browse(ITEMS, "", "platform", "alpha-desc").map((item) => item.uid)).toEqual(["k6", "broker"]);
    expect(browse(ITEMS, "  ", "", "alpha-asc").map((item) => item.uid)).toEqual(["broker", "air", "k6", "scratch"]);
  });

  it("groups by folder in title order and puts General's dashboards last", () => {
    expect(folders(ITEMS, "alpha-asc").map((group) => [group.folder, group.items.map((item) => item.uid)])).toEqual([
      ["Helsinki", ["air"]],
      ["Platform", ["k6", "broker"]],
      [null, ["scratch"]],
    ]);
    expect(folders(ITEMS, "alpha-desc").map((group) => group.folder)).toEqual(["Platform", "Helsinki", null]);
  });

  it("says nothing matches and clears the search and the tag in one step", async () => {
    const user = userEvent.setup();
    renderList();
    await user.selectOptions(await screen.findByLabelText("Tag"), "air");
    await user.type(screen.getByLabelText("Search dashboards"), "no such board");
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("No dashboard matches this search.");
    await user.click(within(status).getByRole("button", { name: "Clear search and filters" }));
    expect(screen.getByLabelText("Search dashboards")).toHaveValue("");
    expect(screen.getByLabelText("Tag")).toHaveValue("");
    expect(screen.getByRole("list", { name: "Dashboards by folder" })).toBeInTheDocument();
  });
});
