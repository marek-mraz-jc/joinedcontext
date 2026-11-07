/**
 * The workspace pages (T-1245, T-1246, T-1248, T-1249, T-1251; UI-61, UI-62, CC-80): opening a
 * copy, the bar and the query parameter, comparing, bringing back with conflicts, and the list.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { WorkOnCopyDialog } from "../src/components/WorkOnCopyDialog";
import { WorkspaceBar } from "../src/components/layout/WorkspaceBar";
import { setActiveWorkspace, workspaceMiddleware } from "../src/components/layout/WorkspaceContext";
import { expectDenied } from "./checks";
import { ComparePage } from "../src/pages/workspaces/ComparePage";
import { BringBackPage } from "../src/pages/workspaces/BringBackPage";
import { WorkspacesPage } from "../src/routes/WorkspacesPage";

const navigate = vi.fn();
let search: Record<string, string> = {};
vi.mock("@tanstack/react-router", () => ({
  Link: ({ children, to, params }: { children: ReactNode; to: string; params?: Record<string, string> }) => (
    <a href={Object.entries(params ?? {}).reduce((path, [k, v]) => path.replace(`$${k}`, v), to)}>{children}</a>
  ),
  useNavigate: () => navigate,
  useRouterState: ({ select }: { select: (s: { location: { search: unknown } }) => unknown }) =>
    select({ location: { search } }),
}));

let me = { email: "jana@hel.fi", username: "jana" };
vi.mock("../src/auth/AuthProvider", () => ({ useAuthStatus: () => "authenticated", useAuth: () => ({ identity: me, status: "authenticated" }) }));

type Handler = (request: Request, url: URL) => Response | Promise<Response> | undefined;
let handler: Handler;
let requests: { method: string; path: string; body: unknown }[];

const json = (body: unknown, status = 200) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "Content-Type": status === 204 ? "text/plain" : "application/json" },
  });

const WORKSPACE = {
  name: "air-v2",
  title: "Air cleanup",
  project: "helsinki",
  owner: "jana@hel.fi",
  branch: "workspace/air-v2",
  baseRevision: "base1",
  scope: { kind: "project" },
  previewState: "none",
  createdAt: "2026-09-18T09:00:00Z",
  // Far ahead: a date that passes turns this copy expired and fails every test that opens it.
  expiresAt: "2099-09-25T09:00:00Z",
  changes: 3,
};
const file = (path: string, kind: string, operation: string, lane = "yellow") => ({
  path,
  kind,
  operation,
  lane,
  fields: [{ path: "spec.period", from: "10s", to: "30s" }],
});

beforeEach(async () => {
  await i18n.changeLanguage("en");
  navigate.mockReset();
  search = {};
  me = { email: "jana@hel.fi", username: "jana" };
  requests = [];
  handler = () => undefined;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(request.url);
      const body = request.method === "GET" ? undefined : await request.clone().json().catch(() => undefined);
      requests.push({ method: request.method, path: url.pathname + url.search, body });
      return (await handler(request, url)) ?? json({ title: "Not Found", status: 404 }, 404);
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  setActiveWorkspace(null);
});

function show(ui: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{ui}</I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("work on a copy", () => {
  it("offers the scope's name, seven days, and posts what was chosen", async () => {
    handler = (req, url) =>
      req.method === "POST" && url.pathname === "/api/v1/projects/helsinki/workspaces" ? json(WORKSPACE, 201) : undefined;
    const onOpenChange = vi.fn();
    show(<WorkOnCopyDialog project="helsinki" scope={{ kind: "space", name: "Air_Quality" }} open onOpenChange={onOpenChange} />);
    expect(screen.getByText("Space Air_Quality")).toBeInTheDocument();
    const name = screen.getByLabelText(/^Name/) as HTMLInputElement;
    expect(name.value).toBe("air-quality");
    expect((screen.getByLabelText("Keep it for") as HTMLSelectElement).value).toBe("7");
    await userEvent.click(screen.getByRole("button", { name: "Start the copy" }));
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(requests.at(-1)?.body).toEqual({
      name: "air-quality",
      ttlDays: 7,
      scope: { kind: "space", name: "Air_Quality" },
    });
  });

  it("refuses an empty, a Unicode or a too long name without asking the server", async () => {
    show(<WorkOnCopyDialog project="helsinki" scope={{ kind: "project" }} open onOpenChange={() => {}} />);
    const name = screen.getByLabelText(/^Name/);
    const start = screen.getByRole("button", { name: "Start the copy" });
    for (const typed of ["", "čistenie", "a".repeat(21)]) {
      await userEvent.clear(name);
      if (typed) await userEvent.type(name, typed);
      // Refused and reachable, with the rule for the name read out beside it (T-1755, UI-44).
      expectDenied(start, "Use lowercase letters, digits and hyphens, at most 20 characters.");
    }
    expect(requests).toEqual([]);
  });

  it("shows the server's reason and stays open when the name is taken", async () => {
    handler = () => json({ title: "Conflict", status: 409, detail: "a workspace named 'copy' already exists" }, 409);
    const onOpenChange = vi.fn();
    show(<WorkOnCopyDialog project="helsinki" scope={{ kind: "project" }} open onOpenChange={onOpenChange} />);
    await userEvent.click(screen.getByRole("button", { name: "Start the copy" }));
    // The taken name is answered at the name field in the dialog's own words (T-3219).
    expect(await screen.findByRole("alert")).toHaveTextContent(/exists already/);
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });
});

describe("the copy's bar and query parameter", () => {
  const through = async (path: string) => {
    const request = new Request(`http://localhost${path}`);
    const out = await workspaceMiddleware.onRequest!({ request } as never);
    return new URL((out as Request).url).search;
  };

  it("adds the copy to resource reads and writes only", async () => {
    expect(await through("/api/v1/projects/helsinki/pipelines")).toBe("");
    setActiveWorkspace("air-v2");
    expect(await through("/api/v1/projects/helsinki/pipelines")).toBe("?workspace=air-v2");
    expect(await through("/api/v1/projects/helsinki/endpoints/bikes?dryRun=All")).toBe("?dryRun=All&workspace=air-v2");
    expect(await through("/api/v1/projects/helsinki/ops/jc_manifest_dry_run")).toBe("");
    expect(await through("/api/v1/projects/helsinki/workspaces/air-v2")).toBe("");
    expect(await through("/api/v1/projects/helsinki/changes")).toBe("");
  });

  // T-2267: unproposed work made inside a copy is that copy's, and the API answers it under the
  // same parameter. Without this the form in a copy read and overwrote the project's draft.
  it("takes the shared drafts of a copy into that copy, but not the project's own stream", async () => {
    setActiveWorkspace("air-v2");
    expect(await through("/api/v1/projects/helsinki/drafts")).toBe("?workspace=air-v2");
    expect(await through("/api/v1/projects/helsinki/drafts/Pipeline/bikes")).toBe("?workspace=air-v2");
    // One stream per project, so the subscription is the project's.
    expect(await through("/api/v1/projects/helsinki/drafts/events")).toBe("");
  });

  it("carries a write into the copy whole: its method, its headers and its body (T-2265)", async () => {
    setActiveWorkspace("air-v2");
    const manifest = JSON.stringify({ kind: "Pipeline", metadata: { name: "bikes" }, spec: { enabled: false } });
    const request = new Request("http://localhost/api/v1/projects/helsinki/pipelines/bikes?dryRun=All", {
      method: "PUT",
      headers: { "content-type": "application/json", "x-csrf-token": "csrf-token-value" },
      body: manifest,
    });
    const out = (await workspaceMiddleware.onRequest!({ request } as never)) as Request;

    expect(out.method).toBe("PUT");
    expect(new URL(out.url).search).toBe("?dryRun=All&workspace=air-v2");
    expect(out.headers.get("x-csrf-token")).toBe("csrf-token-value");
    expect(out.headers.get("content-type")).toBe("application/json");
    // Byte for byte, and as bytes rather than as a stream: a streamed body goes out chunked with
    // no length, which is what the edge refused with 413 (T-2263).
    expect(await out.text()).toBe(manifest);

    // A read carries nothing but the parameter.
    const read = new Request("http://localhost/api/v1/projects/helsinki/pipelines");
    const passed = (await workspaceMiddleware.onRequest!({ request: read } as never)) as Request;
    expect(passed.method).toBe("GET");
    expect(passed.body).toBeNull();
  });

  it("says nothing outside a copy, and what the copy is inside one", async () => {
    const { container } = show(<WorkspaceBar project="helsinki" />);
    expect(container).toBeEmptyDOMElement();
    search = { workspace: "air-v2" };
    handler = (_req, url) => (url.pathname.endsWith("/workspaces/air-v2") ? json(WORKSPACE) : undefined);
    const { WorkspaceProvider } = await import("../src/components/layout/WorkspaceContext");
    show(
      <WorkspaceProvider>
        <WorkspaceBar project="helsinki" />
      </WorkspaceProvider>,
    );
    await waitFor(() => expect(screen.queryByTestId("workspace-bar-loading")).toBeNull());
    const bar = screen.getByRole("region", { name: "Copy" });
    expect(bar.textContent).toContain("Air cleanup");
    expect(bar.textContent).toContain("3 changes");
    expect(within(bar).getByText("Bring back").closest("a")?.getAttribute("href")).toBe(
      "/projects/helsinki/workspaces/air-v2/bring-back",
    );
  });
});

describe("the copy bar's states (T-1253, UI-61)", () => {
  async function barFor(response: Response) {
    search = { workspace: "air-v2" };
    handler = (_req, url) => (url.pathname.endsWith("/workspaces/air-v2") ? response : undefined);
    const { WorkspaceProvider } = await import("../src/components/layout/WorkspaceContext");
    show(
      <WorkspaceProvider>
        <WorkspaceBar project="helsinki" />
      </WorkspaceProvider>,
    );
    // The steady bar is a landmark; the line that replaces it when the copy is gone is a
    // `status`, so that it is announced to somebody reading elsewhere on the page (T-1254).
    // Both carry the bar's label, which is what finds either of them here, once the frame drawn
    // while the copy is read has given way to what the read answered (T-1489).
    await waitFor(() => expect(screen.queryByTestId("workspace-bar-loading")).toBeNull());
    return screen.getByLabelText("Copy");
  }

  // UI-61: the name is in the address before the copy's record is read, so the bar is there from
  // the first frame, busy, with the way out — not absent until the request answers, which made
  // the page jump and let a person start working without seeing they were in a copy (T-1489).
  it("shows_the_copy_name_while_loading", async () => {
    search = { workspace: "air-v2" };
    handler = (_req, url) =>
      url.pathname.endsWith("/workspaces/air-v2") ? new Promise<Response>(() => {}) : undefined;
    const { WorkspaceProvider } = await import("../src/components/layout/WorkspaceContext");
    show(
      <WorkspaceProvider>
        <WorkspaceBar project="helsinki" />
      </WorkspaceProvider>,
    );
    const bar = screen.getByRole("region", { name: "Copy" });
    expect(bar).toHaveAttribute("aria-busy", "true");
    expect(bar.textContent).toContain("air-v2");
    expect(bar.textContent).toContain("Nothing here is live");
    expect(within(bar).getByRole("button", { name: "Leave" })).toBeEnabled();
  });

  it("says nothing changed yet and offers no bring back on an empty copy", async () => {
    const bar = await barFor(json({ ...WORKSPACE, changes: 0 }));
    expect(bar.textContent).toContain("No changes yet");
    expect(within(bar).queryByText("Bring back")).toBeNull();
    expect(within(bar).getByText("Compare")).toBeInTheDocument();
  });

  it("names the day an expired copy ended and never what it held", async () => {
    const bar = await barFor(json({ ...WORKSPACE, expiresAt: "2026-01-02T09:00:00Z" }));
    expect(bar.textContent).toContain("This copy expired on");
    expect(bar.textContent).toContain("never brought back");
    expect(bar.textContent).not.toContain("Air cleanup");
    expect(within(bar).getByRole("button", { name: "Leave" })).toBeInTheDocument();
  });

  it("gives the reason when the copy is not the caller's to see", async () => {
    const bar = await barFor(json({ title: "Forbidden", status: 403, detail: "no grant in helsinki" }, 403));
    expect(bar.textContent).toContain("You may not open this copy: no grant in helsinki");
  });

  it("says a gone copy is gone", async () => {
    const bar = await barFor(json({ title: "Not Found", status: 404 }, 404));
    expect(bar.textContent).toContain("expired or was discarded");
  });

  it("marks someone else's copy read only and leaves bring back to its owner", async () => {
    me = { email: "petra@hel.fi", username: "petra" };
    const bar = await barFor(json(WORKSPACE));
    expect(within(bar).getByTestId("workspace-foreign").textContent).toBe("Read only: the copy of jana@hel.fi");
    expect(within(bar).queryByText("Bring back")).toBeNull();
  });
});

describe("compare", () => {
  it("groups by kind, lists added before changed before removed, and folds the fields", async () => {
    handler = (_req, url) =>
      url.pathname.endsWith("/compare")
        ? json({
            files: [
              file("projects/helsinki/pipelines/old.yaml", "Pipeline", "Delete", "red"),
              file("projects/helsinki/pipelines/bikes.yaml", "Pipeline", "Update"),
              file("projects/helsinki/pipelines/new.yaml", "Pipeline", "Create"),
              file("projects/helsinki/spaces/air/space.yaml", "ContextSpace", "Update"),
            ],
            conflicts: [],
          })
        : undefined;
    show(<ComparePage project="helsinki" name="air-v2" />);
    const pipelines = (await screen.findByRole("heading", { name: "Pipeline" })).closest("section")!;
    const names = within(pipelines).getAllByText(/^(old|bikes|new)$/).map((n) => n.textContent);
    expect(names).toEqual(["new", "bikes", "old"]);
    expect(screen.getByText("1 added, 2 changed, 1 removed")).toBeInTheDocument();
    const details = pipelines.querySelector("details")!;
    expect(details).not.toHaveAttribute("open");
    expect(screen.queryByRole("status", { name: /also changed/ })).toBeNull();
    // The way on is on the page itself, not only in the copy's bar (T-4868).
    expect(screen.getByRole("link", { name: "Bring back" })).toHaveAttribute(
      "href",
      expect.stringMatching(/^\/projects\/helsinki\/workspaces\/air-v2\/bring-back/),
    );
  });

  it("names a resource kept in a folder of its own by the folder, not its file (T-4868)", async () => {
    handler = (_req, url) =>
      url.pathname.endsWith("/compare")
        ? json({
            files: [
              file("projects/helsinki/pipelines/air-quality/pipeline.yaml", "Pipeline", "Update"),
              file("projects/helsinki/datasources/fmi-air-quality.yaml", "DataSource", "Update"),
            ],
            conflicts: [],
          })
        : undefined;
    show(<ComparePage project="helsinki" name="air-v2" />);
    expect(await screen.findByText("air-quality")).toBeInTheDocument();
    expect(screen.getByText("fmi-air-quality")).toBeInTheDocument();
    expect(screen.queryByText("pipeline")).toBeNull();
  });

  it("points at the bring back page when the project changed the same files", async () => {
    handler = (_req, url) =>
      url.pathname.endsWith("/compare")
        ? json({ files: [file("projects/helsinki/pipelines/a.yaml", "Pipeline", "Update")], conflicts: [{ path: "projects/helsinki/pipelines/a.yaml", fields: [] }] })
        : undefined;
    show(<ComparePage project="helsinki" name="air-v2" />);
    expect(await screen.findByText(/1 file also changed in the project/)).toBeInTheDocument();
    expect(screen.getByText("Resolve on the bring back page").closest("a")?.getAttribute("href")).toBe(
      "/projects/helsinki/workspaces/air-v2/bring-back",
    );
  });

  it("says so when the copy changes nothing", async () => {
    handler = (_req, url) => (url.pathname.endsWith("/compare") ? json({ files: [], conflicts: [] }) : undefined);
    show(<ComparePage project="helsinki" name="air-v2" />);
    expect(await screen.findByText("The copy changes nothing yet.")).toBeInTheDocument();
  });

  // UI-62, T-1503: a comparison the API refuses says the API's own sentence, never an empty copy.
  it("says the API's reason when the comparison fails, and not that the copy changes nothing", async () => {
    handler = (_req, url) =>
      url.pathname.endsWith("/compare")
        ? json({ title: "Service Unavailable", status: 503, detail: "git forge is not configured" }, 503)
        : undefined;
    show(<ComparePage project="helsinki" name="air-v2" />);
    expect(await screen.findByRole("alert")).toHaveTextContent("git forge is not configured");
    expect(screen.queryByText("The copy changes nothing yet.")).toBeNull();
  });
});

describe("bring back", () => {
  const CONFLICT = {
    files: [file("projects/helsinki/pipelines/a.yaml", "Pipeline", "Update", "red")],
    conflicts: [
      {
        path: "projects/helsinki/pipelines/a.yaml",
        fields: [
          { path: "spec.period", ours: "30s", theirs: "60s", base: "10s" },
          { path: "spec.note", ours: "<b>x</b>", theirs: "y", base: "z" },
        ],
      },
    ],
  };
  let compare: unknown;
  beforeEach(() => {
    compare = CONFLICT;
    handler = (req, url) => {
      if (url.pathname.endsWith("/compare")) return json(compare);
      if (url.pathname.endsWith("/update")) {
        compare = { files: CONFLICT.files, conflicts: [] };
        return json({ taken: [], merged: [CONFLICT.files[0].path], baseRevision: "b2", comparison: compare });
      }
      if (url.pathname.endsWith("/propose"))
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-00000009", namespace: "helsinki" }, status: { lane: "red", phase: "PendingApproval" } }, 202);
      if (url.pathname.endsWith("/workspaces/air-v2") && req.method === "GET") return json(WORKSPACE);
      return undefined;
    };
  });

  it("holds the proposal until every conflicting field has a side, then updates with the choices", async () => {
    show(<BringBackPage project="helsinki" name="air-v2" />);
    const propose = await screen.findByRole("button", { name: "Propose as one change" });
    const update = screen.getByRole("button", { name: "Update from the project" });
    // Refused with a reason rather than hard-disabled (T-1743, UI-44): a `disabled` button
    // leaves the tab order and the sentence saying what is still missing can then never be read.
    expectDenied(propose, /Resolve the files the project changed too/);
    expectDenied(update, /Answer every field that the project changed too/);
    expect(screen.getByText("Red lane: the approver types the name back to confirm.")).toBeInTheDocument();
    // A value renders as text, never as markup.
    expect(screen.getByText("<b>x</b>")).toBeInTheDocument();
    expect(document.querySelector("b")).toBeNull();

    const period = screen.getByRole("radiogroup", { name: "spec.period" });
    await userEvent.click(within(period).getByLabelText(/Keep the copy's/));
    expectDenied(update, /Answer every field that the project changed too/);
    const note = screen.getByRole("radiogroup", { name: "spec.note" });
    await userEvent.click(within(note).getByLabelText(/Take the project's/));
    await userEvent.click(update);
    await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled"));
    // The update says what it did, or a person clicks and sees nothing happen (T-2729).
    expect(screen.getByRole("status")).toHaveTextContent("Updated from the project: 1 file taken in.");
    expect(requests.find((r) => r.path.endsWith("/update"))?.body).toEqual({
      resolutions: [
        { path: "projects/helsinki/pipelines/a.yaml", field: "spec.period", keep: "ours" },
        { path: "projects/helsinki/pipelines/a.yaml", field: "spec.note", keep: "theirs" },
      ],
    });

    await userEvent.click(propose);
    const link = await screen.findByText("chg-00000009");
    expect(link.closest("a")?.getAttribute("href")).toBe("/projects/helsinki/approvals/chg-00000009");
  });

  it("says so when the project changed nothing the copy does not already have", async () => {
    compare = { files: CONFLICT.files, conflicts: [] };
    handler = (req, url) => {
      if (url.pathname.endsWith("/compare")) return json(compare);
      if (url.pathname.endsWith("/update")) return json({ taken: [], merged: [], baseRevision: "b1", comparison: compare });
      if (url.pathname.endsWith("/workspaces/air-v2") && req.method === "GET") return json(WORKSPACE);
      return undefined;
    };
    show(<BringBackPage project="helsinki" name="air-v2" />);
    await userEvent.click(await screen.findByRole("button", { name: "Update from the project" }));
    expect(await screen.findByRole("status")).toHaveTextContent("The copy already has everything the project changed.");
  });

  // UI-62, T-1503: a bring back whose comparison fails offers no proposal built on nothing.
  it("says the API's reason when the comparison fails, and offers nothing to propose", async () => {
    handler = (req, url) => {
      if (url.pathname.endsWith("/compare"))
        return json({ title: "Forbidden", status: 403, detail: "no role grants propose on Pipeline in project helsinki (PF-50)" }, 403);
      if (url.pathname.endsWith("/workspaces/air-v2") && req.method === "GET") return json(WORKSPACE);
      return undefined;
    };
    show(<BringBackPage project="helsinki" name="air-v2" />);
    expect(await screen.findByRole("alert")).toHaveTextContent("no role grants propose on Pipeline");
    expect(screen.queryByRole("button", { name: "Propose as one change" })).toBeNull();
    expect(requests.some((r) => r.method === "POST")).toBe(false);
  });

  it("gives someone else's copy no buttons", async () => {
    me = { email: "petra@hel.fi", username: "petra" };
    compare = { files: CONFLICT.files, conflicts: [] };
    show(<BringBackPage project="helsinki" name="air-v2" />);
    expect(await screen.findByText("Only the person who started this copy brings it back.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Propose as one change" })).toBeNull();
  });
});

describe("the list of copies", () => {
  const other = { ...WORKSPACE, name: "bikes", title: undefined, owner: "petra@hel.fi" };
  it("splits mine from other people's, and discards mine after asking", async () => {
    let items = [WORKSPACE, other];
    handler = (req, url) => {
      if (req.method === "DELETE") {
        items = [other];
        return new Response(null, { status: 204 });
      }
      return url.pathname.endsWith("/workspaces") ? json({ items }) : undefined;
    };
    show(<WorkspacesPage project="helsinki" />);
    const mine = (await screen.findByRole("heading", { name: "My copies" })).closest("section")!;
    const others = screen.getByRole("heading", { name: "Other people's copies" }).closest("section")!;
    expect(within(mine).getByText("Air cleanup")).toBeInTheDocument();
    expect(within(others).getByText("bikes")).toBeInTheDocument();
    expect(within(others).queryByRole("button", { name: "Discard" })).toBeNull();
    await userEvent.click(within(mine).getByRole("button", { name: "Discard" }));
    // The shared dialog asks, names the copy, and nothing is deleted until it is answered.
    const asking = await screen.findByRole("alertdialog");
    expect(asking).toHaveAccessibleDescription(expect.stringContaining("air-v2"));
    expect(requests.some((r) => r.method === "DELETE")).toBe(false);

    await userEvent.click(within(asking).getByTestId("confirm-accept"));
    await waitFor(() => expect(screen.queryByText("Air cleanup")).toBeNull());
    expect(requests.some((r) => r.method === "DELETE" && r.path.endsWith("/workspaces/air-v2"))).toBe(true);
  });

  it("opens a copy at the project's spaces with the copy in the address", async () => {
    handler = (_req, url) => (url.pathname.endsWith("/workspaces") ? json({ items: [WORKSPACE] }) : undefined);
    show(<WorkspacesPage project="helsinki" />);
    await userEvent.click(await screen.findByRole("button", { name: "Open" }));
    expect(navigate).toHaveBeenCalledWith(
      expect.objectContaining({ params: { project: "helsinki", plural: "spaces" }, search: { workspace: "air-v2" } }),
    );
  });

  it("says what a copy is for when there is none", async () => {
    handler = (_req, url) => (url.pathname.endsWith("/workspaces") ? json({ items: [] }) : undefined);
    show(<WorkspacesPage project="helsinki" />);
    expect(await screen.findByText(/No copies/)).toBeInTheDocument();
  });
});
