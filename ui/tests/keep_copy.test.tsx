/**
 * T-4868: a person inside a copy stays in it on every in-app link. The pipeline page's Edit went
 * to the editor without `?workspace=`, the copy bar vanished and Propose wrote to the project
 * while the person still believed they were working on the copy. Only Leave, and switching to
 * another project, take the copy away.
 */
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { describe, expect, it } from "vitest";
import { keepCopy } from "../src/components/layout/keepCopy";
import { withoutCopy } from "../src/components/layout/WorkspaceContext";

function routerAt(url: string) {
  const root = createRootRoute();
  const inside = createRoute({
    getParentRoute: () => root,
    id: "protected",
    validateSearch: (search: Record<string, unknown>): { workspace?: string } => ({
      workspace: typeof search.workspace === "string" && search.workspace !== "" ? search.workspace : undefined,
    }),
    search: { middlewares: [keepCopy] },
  });
  const list = createRoute({ getParentRoute: () => inside, path: "/projects/$project/$plural" });
  const edit = createRoute({ getParentRoute: () => inside, path: "/projects/$project/$plural/$name/edit" });
  inside.addChildren([list, edit]);
  return createRouter({
    routeTree: root.addChildren([inside]),
    history: createMemoryHistory({ initialEntries: [url] }),
  });
}

const workspaceAfter = async (router: ReturnType<typeof routerAt>, go: () => Promise<void>) => {
  await router.load();
  await go();
  return (router.state.location.search as { workspace?: string }).workspace;
};

describe("keepCopy", () => {
  it("keeps the copy on a link that does not name it", async () => {
    const router = routerAt("/projects/helsinki/pipelines?workspace=air-v2");
    const workspace = await workspaceAfter(router, () =>
      router.navigate({
        to: "/projects/$project/$plural/$name/edit",
        params: { project: "helsinki", plural: "pipelines", name: "air-quality" },
      }),
    );
    expect(workspace).toBe("air-v2");
    expect(router.state.location.pathname).toBe("/projects/helsinki/pipelines/air-quality/edit");
  });

  it("leaves the copy when the link says so", async () => {
    const router = routerAt("/projects/helsinki/pipelines?workspace=air-v2");
    const workspace = await workspaceAfter(router, () =>
      router.navigate({ to: ".", search: (prev: Record<string, unknown>) => withoutCopy(prev) }),
    );
    expect(workspace).toBeUndefined();
    expect(router.state.location.href).toBe("/projects/helsinki/pipelines");
  });

  it("leaves the copy for another project, whose copies have other names", async () => {
    const router = routerAt("/projects/helsinki/pipelines?workspace=air-v2");
    const workspace = await workspaceAfter(router, () =>
      router.navigate({
        to: "/projects/$project/$plural",
        params: { project: "tampere", plural: "pipelines" },
        search: withoutCopy({}),
      }),
    );
    expect(workspace).toBeUndefined();
  });

  it("enters no copy on a page outside one", async () => {
    const router = routerAt("/projects/helsinki/pipelines");
    const workspace = await workspaceAfter(router, () =>
      router.navigate({
        to: "/projects/$project/$plural/$name/edit",
        params: { project: "helsinki", plural: "pipelines", name: "air-quality" },
      }),
    );
    expect(workspace).toBeUndefined();
  });
});
