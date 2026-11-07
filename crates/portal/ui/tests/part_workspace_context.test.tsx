/**
 * T-1823: the provider that puts the page inside a copy, against the UI contract (UI-15, UI-16).
 *
 * It renders nothing of its own, so the contract here is what it does to everything below it:
 * the copy is known before the first child fetches, entering and leaving read the project again
 * instead of showing what was cached for the other one, and the URL keeps whatever else it held.
 * A page rendered outside it is in the project, never in a copy by accident.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  WorkspaceProvider,
  activeWorkspace,
  setActiveWorkspace,
  useWorkspace,
  workspaceMiddleware,
} from "../src/components/layout/WorkspaceContext";

const navigate = vi.fn();
let search: Record<string, string> = {};
vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => navigate,
  useRouterState: ({ select }: { select: (s: { location: { search: unknown } }) => unknown }) =>
    select({ location: { search } }),
}));

/** What a child sees, and what the first request of a page below it would have carried. */
function Child(): React.JSX.Element {
  const { name, enter, leave } = useWorkspace();
  useEffect(() => {
    document.body.dataset.atFirstFetch = activeWorkspace() ?? "";
  }, []);
  return (
    <div>
      <p data-testid="name">{name ?? "the project"}</p>
      <button type="button" onClick={() => enter("air-v2")}>
        enter
      </button>
      <button type="button" onClick={leave}>
        leave
      </button>
    </div>
  );
}

function show(children: ReactNode = <Child />) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidate = vi.spyOn(client, "invalidateQueries");
  // A fresh element each time: React bails out of a re-render handed the very same one.
  const tree = () => (
    <QueryClientProvider client={client}>
      <WorkspaceProvider>{children}</WorkspaceProvider>
    </QueryClientProvider>
  );
  const view = render(tree());
  /** Render again, so the provider reads the URL the test has just changed. */
  const again = () => view.rerender(tree());
  return { ...view, invalidate, again };
}

const through = async (url: string) => {
  const out = (await workspaceMiddleware.onRequest!({
    request: new Request(`http://localhost${url}`),
  } as never)) as Request;
  return new URL(out.url).search;
};

beforeEach(() => {
  navigate.mockReset();
  search = {};
  delete document.body.dataset.atFirstFetch;
  setActiveWorkspace(null);
});

afterEach(() => {
  setActiveWorkspace(null);
});

describe("the workspace provider against the UI contract", () => {
  it("adds nothing to the page and nothing to the tab order", () => {
    search = { workspace: "air-v2" };
    const { container } = show(<p>only this</p>);

    expect(container.innerHTML).toBe("<p>only this</p>");
  });

  it("knows the copy before the first child fetches, not one effect later", () => {
    search = { workspace: "air-v2" };
    show();

    expect(document.body.dataset.atFirstFetch).toBe("air-v2");
    expect(screen.getByTestId("name").textContent).toBe("air-v2");
  });

  it("reads everything again when the copy changes, and when it is left", async () => {
    const { invalidate, again } = show();
    // Mounting is the first read either way: there is nothing cached to throw away yet.
    expect(invalidate).not.toHaveBeenCalled();

    search = { workspace: "air-v2" };
    again();
    await waitFor(() => expect(invalidate).toHaveBeenCalledTimes(1));
    expect(screen.getByTestId("name").textContent).toBe("air-v2");

    // Leaving is the same move in the other direction: what was cached belongs to the copy.
    search = {};
    again();
    await waitFor(() => expect(invalidate).toHaveBeenCalledTimes(2));
    expect(screen.getByTestId("name").textContent).toBe("the project");
  });

  it("keeps the rest of the URL when it enters and when it leaves", async () => {
    search = { page: "2", q: "pm10" };
    const user = userEvent.setup();
    show();

    await user.click(screen.getByRole("button", { name: "enter" }));
    const entering = navigate.mock.calls[0][0] as { to: string; search: (p: object) => object };
    expect(entering.to).toBe(".");
    expect(entering.search({ page: "2", q: "pm10" })).toEqual({ page: "2", q: "pm10", workspace: "air-v2" });

    await user.click(screen.getByRole("button", { name: "leave" }));
    const leaving = navigate.mock.calls[1][0] as { search: (p: object) => object };
    expect(leaving.search({ page: "2", q: "pm10", workspace: "air-v2" })).toEqual({ page: "2", q: "pm10" });
  });

  it("is the project, not a copy, for a page rendered outside it", () => {
    const client = new QueryClient();
    render(
      <QueryClientProvider client={client}>
        <Child />
      </QueryClientProvider>,
    );

    expect(screen.getByTestId("name").textContent).toBe("the project");
    expect(activeWorkspace()).toBeNull();
  });

  it("puts the drafts of a copy inside it and leaves the project's own stream alone", async () => {
    setActiveWorkspace("air-v2");

    expect(await through("/api/v1/projects/helsinki/drafts")).toBe("?workspace=air-v2");
    expect(await through("/api/v1/projects/helsinki/drafts/endpoints/bikes")).toBe("?workspace=air-v2");
    // A copy has no event stream of its own; asking for one under `?workspace=` would hang.
    expect(await through("/api/v1/projects/helsinki/drafts/events")).toBe("");
  });

  it("replaces a workspace parameter a caller wrote by hand, never appends a second one", async () => {
    setActiveWorkspace("air-v2");

    expect(await through("/api/v1/projects/helsinki/pipelines?workspace=other")).toBe("?workspace=air-v2");
  });

  it("puts every project kind jc-core writes inside the copy, the assistant's and MCP kinds too", async () => {
    setActiveWorkspace("air-v2");

    for (const plural of ["knowledgesources", "assistantdeployments", "mcpservers", "contextbrokers", "delegationpolicies"]) {
      expect(await through(`/api/v1/projects/helsinki/${plural}`)).toBe("?workspace=air-v2");
      expect(await through(`/api/v1/projects/helsinki/${plural}/one`)).toBe("?workspace=air-v2");
    }
  });

  it("keeps the caller's abort signal on the request it rebuilds", async () => {
    setActiveWorkspace("air-v2");
    const controller = new AbortController();
    const out = (await workspaceMiddleware.onRequest!({
      request: new Request("http://localhost/api/v1/projects/helsinki/pipelines", { signal: controller.signal }),
    } as never)) as Request;

    controller.abort();

    expect(out.url).toContain("?workspace=air-v2");
    expect(out.signal.aborted).toBe(true);
  });
});
