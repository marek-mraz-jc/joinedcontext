/**
 * T-3986: the rows the dialog walk may fill (scripts/walk-rows.ts): a module counts only when a
 * page the walk opened draws it, that page had no finding, the walk never enters Shell, and a
 * kind's own folder counts only when that kind was walked.
 */
import { describe, expect, it } from "vitest";
import { closure, routeOf, routes, rows, shellRows } from "../scripts/walk-rows";
import type { Graph } from "../scripts/walk-rows";

const ROUTER = `
import { Shell } from "./components/layout/Shell";
import { NotFound } from "./components/NotFound";
const SpacesPage = lazy(() => import("./pages/spaces/SpacesPage").then((m) => ({ default: m.SpacesPage })));
const AppsPage = lazy(() => import("./pages/apps/AppsPage").then((m) => ({ default: m.AppsPage })));
const PeoplePage = lazy(() => import("./pages/organization/People").then((m) => ({ default: m.PeoplePage })));

function KindPage({
  plural,
}: {
  plural: string;
}) {
  return plural === "spaces" ? <SpacesPage /> : <AppsPage />;
}

const peopleRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/organization/people",
  component: function PeopleRoute() {
    return <Shell><PeoplePage /></Shell>;
  },
});

const sectionRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/$plural",
  component: function SectionRoute() {
    return <Shell><KindPage plural="spaces" /><NotFound /></Shell>;
  },
});

const sectionDetailRoute = createRoute({
  getParentRoute: () => sectionRoute,
  path: "$name",
  component: Nothing,
});
`;

const graph: Graph = new Map([
  ["src/components/layout/Shell.tsx", new Set(["src/navigation/Nav.tsx"])],
  ["src/navigation/Nav.tsx", new Set()],
  ["src/pages/spaces/SpacesPage.tsx", new Set(["src/components/ui/Button.tsx"])],
  ["src/pages/apps/AppsPage.tsx", new Set(["src/components/ui/Button.tsx"])],
  ["src/pages/organization/People.tsx", new Set(["src/components/InviteDialog.tsx"])],
  ["src/components/InviteDialog.tsx", new Set()],
  ["src/components/ui/Button.tsx", new Set()],
]);
const folders = new Set(["spaces", "apps", "organization"]);
const fixture = (modules: string[]) => ({
  name: "fixture",
  path: "",
  modules: new Set(modules),
});

describe("walk rows (T-3986)", () => {
  it("reads full paths, children through their parent, and the router's own functions", () => {
    expect(routes(ROUTER).map((route) => route.path)).toEqual([
      "/organization/people",
      "/projects/$project/$plural",
      "/projects/$project/$plural/$name",
    ]);
    expect(routeOf("/projects/helsinki/spaces/air?lang=en", routes(ROUTER))?.path).toBe("/projects/$project/$plural/$name");
    expect(routeOf("/organization/nobody/here", routes(ROUTER))).toBeUndefined();
  });

  it("never enters Shell", () => {
    expect([...closure(graph, ["src/components/layout/Shell.tsx", "src/pages/spaces/SpacesPage.tsx"])].sort()).toEqual([
      "src/components/ui/Button.tsx",
      "src/pages/spaces/SpacesPage.tsx",
    ]);
  });

  it("fills what a clean page draws, with its screenshots, and nothing a failed page or an unwalked kind draws", () => {
    const routed = [
      { ...fixture(["src/components/layout/Shell.tsx", "src/pages/organization/People.tsx"]), path: "/organization/people" },
      {
        ...fixture(["src/components/layout/Shell.tsx", "src/pages/spaces/SpacesPage.tsx", "src/pages/apps/AppsPage.tsx"]),
        path: "/projects/$project/$plural",
      },
    ];
    const found = rows(
      {
        walked: ["/projects/helsinki/spaces", "/organization/people"],
        report: [{ address: "/organization/people", findings: ["escape: stays open"] }],
        shots: [
          { file: "spaces-dialog.png", route: "/projects/helsinki/spaces" },
          { file: "people-dialog.png", route: "/organization/people" },
        ],
      },
      routed,
      graph,
      folders,
    );
    expect([...found.keys()].sort()).toEqual(["src/components/ui/Button.tsx", "src/pages/spaces/SpacesPage.tsx"]);
    expect(found.get("src/pages/spaces/SpacesPage.tsx")).toEqual(["spaces-dialog.png"]);
  });

  it("fills what Shell draws from a clean shell pass, and nothing from one with a finding", () => {
    const shots = [
      { file: "shell-language.png", route: "/projects/helsinki/spaces", state: "shell-language" },
      { file: "spaces-dialog.png", route: "/projects/helsinki/spaces", state: "dialog-new" },
    ];
    const clean = shellRows({ report: [{ address: "/projects/helsinki/spaces", findings: [] }], shots }, graph);
    expect([...clean.keys()]).toContain("src/components/layout/Shell.tsx");
    expect(clean.get("src/components/layout/Shell.tsx")).toEqual(["shell-language.png"]);
    expect(shellRows({ report: [{ address: "/projects/helsinki/spaces", findings: ["axe: x"] }], shots }, graph).size).toBe(0);
    expect(shellRows({ report: [], shots }, graph).size).toBe(0);
  });

  it("fills nothing from an empty walk", () => {
    expect(rows({ walked: [], report: [], shots: [] }, routes(ROUTER), graph, folders).size).toBe(0);
  });
});
