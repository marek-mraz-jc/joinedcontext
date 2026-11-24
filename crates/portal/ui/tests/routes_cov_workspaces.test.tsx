/**
 * The copies list's remaining branches: refusals and retries, the discard dialog's cancel and
 * failure, the dialog address `/workspaces/new`, the grid's scope, preview and expiry columns.
 */
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { WorkspacesPage } from "../src/routes/WorkspacesPage";

const navigate = vi.fn();
vi.mock("@tanstack/react-router", () => ({
  Link: ({ children, to, params }: { children: ReactNode; to: string; params?: Record<string, string> }) => (
    <a href={Object.entries(params ?? {}).reduce((path, [k, v]) => path.replace(`$${k}`, v), to)}>{children}</a>
  ),
  useNavigate: () => navigate,
  useRouterState: ({ select }: { select: (s: { location: { search: unknown } }) => unknown }) =>
    select({ location: { search: {} } }),
}));
vi.mock("../src/auth/AuthProvider", () => ({
  useAuthStatus: () => "authenticated",
  useAuth: () => ({ identity: { email: "jana@hel.fi", username: "jana" }, status: "authenticated" }),
}));

type Handler = (request: Request, url: URL) => Response | undefined;
let handler: Handler;
let requests: { method: string; path: string }[];

const json = (body: unknown, status = 200) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
  });

const MINE = {
  name: "air-v2",
  title: "Air cleanup",
  project: "helsinki",
  owner: "jana@hel.fi",
  branch: "workspace/air-v2",
  baseRevision: "base1",
  scope: { kind: "project" },
  previewState: "none",
  createdAt: "2026-09-18T09:00:00Z",
  expiresAt: "2099-09-25T09:00:00Z",
  changes: 3,
};
const SPACE = { ...MINE, name: "bikes", title: undefined, owner: "petra@hel.fi", scope: { kind: "space", name: "mobility" }, previewState: "running" };
const ITEMS = {
  ...MINE,
  name: "kit",
  title: undefined,
  owner: "petra@hel.fi",
  scope: { kind: "resources", items: [{ kind: "Endpoint", name: "air" }, { kind: "Pipeline", name: "feed" }] },
  previewState: "error",
};
const NOSCOPE = { ...MINE, name: "blank", title: undefined, owner: "ivan@hel.fi", scope: undefined, previewState: "weird" };

beforeEach(async () => {
  await i18n.changeLanguage("en");
  navigate.mockReset();
  requests = [];
  handler = () => undefined;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(request.url);
      requests.push({ method: request.method, path: url.pathname });
      return handler(request, url) ?? json({ title: "Not Found", status: 404 }, 404);
    }),
  );
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function show(ui: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{ui}</I18nextProvider>
    </QueryClientProvider>,
  );
}

const MAY = { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["*"], verbs: ["read", "propose"] } }] };
const lists = (items: unknown[]): Handler => (_req, url) => {
  if (url.pathname.endsWith("/permissions/me")) return json(MAY);
  return url.pathname.endsWith("/workspaces") ? json({ items }) : undefined;
};

describe("copies list, remaining branches", () => {
  it("says the copies are loading while the list is on its way", async () => {
    handler = () => undefined;
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>(() => undefined)));
    show(<WorkspacesPage project="helsinki" />);
    const status = await screen.findByRole("status", { name: en.app.loading });
    expect(status).toHaveAttribute("aria-busy", "true");
    expect(screen.queryByText(en.workspaces.empty)).toBeNull();
  });

  it("shows the reason the list failed with a Retry that asks again", async () => {
    let calls = 0;
    handler = (_req, url) => {
      if (!url.pathname.endsWith("/workspaces")) return undefined;
      calls += 1;
      return calls === 1 ? json({ title: "Unavailable", status: 503, detail: "The copy store is away." }, 503) : json({ items: [MINE] });
    };
    show(<WorkspacesPage project="helsinki" />);
    expect(await screen.findByRole("alert")).toHaveTextContent("The copy store is away.");
    expect(screen.queryByText(en.workspaces.empty)).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByText("Air cleanup")).toBeInTheDocument();
    expect(calls).toBe(2);
  });

  it("says a hidden project is out of reach, with no retry", async () => {
    handler = (_req, url) =>
      url.pathname.endsWith("/workspaces") ? json({ title: "Not Found", status: 404, detail: "project 'helsinki' not found" }, 404) : undefined;
    show(<WorkspacesPage project="helsinki" />);
    expect(await screen.findByText(en.form.projectHidden)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.app.error.retry })).toBeNull();
  });

  it("offers a first copy from the empty state", async () => {
    handler = lists([]);
    show(<WorkspacesPage project="helsinki" />);
    const link = await screen.findByRole("link", { name: en.workspaces.new });
    expect(link).toHaveAttribute("href", "/projects/helsinki/workspaces/new");
    expect(screen.queryByRole("heading", { name: en.workspaces.mine })).toBeNull();
  });

  it("shows only the section that has copies", async () => {
    handler = lists([SPACE]);
    show(<WorkspacesPage project="helsinki" />);
    expect(await screen.findByRole("heading", { name: en.workspaces.othersTitle })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: en.workspaces.mine })).toBeNull();
  });

  it("describes what each copy covers, its preview and its expiry", async () => {
    handler = lists([MINE, SPACE, ITEMS, NOSCOPE]);
    show(<WorkspacesPage project="helsinki" />);
    const mine = (await screen.findByRole("heading", { name: en.workspaces.mine })).closest("section") as HTMLElement;
    expect(within(mine).getByText(en.workspaces.scopeProject)).toBeInTheDocument();
    expect(within(mine.querySelector("tbody") as HTMLElement).getByText(en.workspaces.previewStates.none)).toBeInTheDocument();
    const time = mine.querySelector("time");
    expect(time).toHaveAttribute("datetime", "2099-09-25T09:00:00Z");
    expect(time?.textContent).toContain("2099");
    const others = (screen.getByRole("heading", { name: en.workspaces.othersTitle }).closest("section") as HTMLElement).querySelector("tbody") as HTMLElement;
    expect(within(others).getByText("Space mobility")).toBeInTheDocument();
    expect(within(others).getByText("Endpoint/air, Pipeline/feed")).toBeInTheDocument();
    expect(within(others).getByText(en.workspaces.previewStates.running)).toBeInTheDocument();
    expect(within(others).getByText(en.workspaces.previewStates.error)).toBeInTheDocument();
    expect(within(others).getByText("ivan@hel.fi")).toBeInTheDocument();
  });

  it("finds a copy by its title or name, and filters by owner and preview state", async () => {
    handler = lists([SPACE, ITEMS, NOSCOPE]);
    show(<WorkspacesPage project="helsinki" />);
    const table = await screen.findByRole("table", { name: en.workspaces.tableCaption });
    const user = userEvent.setup();
    const rows = () => within(table).getAllByRole("row").length - 1;
    expect(rows()).toBe(3);
    await user.type(screen.getByRole("searchbox"), "kit");
    expect(rows()).toBe(1);
    expect(within(table).getByText("kit")).toBeInTheDocument();
    await user.clear(screen.getByRole("searchbox"));
    await user.click(screen.getByText(en.grid.filter));
    await user.click(screen.getByRole("checkbox", { name: /petra@hel\.fi/ }));
    expect(rows()).toBe(2);
    expect(within(table).queryByText("blank")).toBeNull();
  });

  it("opens a copy's compare page by a link and its spaces by the button", async () => {
    handler = lists([MINE]);
    show(<WorkspacesPage project="helsinki" />);
    const compare = await screen.findByRole("link", { name: en.workspaces.compareAction });
    expect(compare).toHaveAttribute("href", "/projects/helsinki/workspaces/air-v2/compare");
  });

  it("leaves the copy alone when the discard dialog is cancelled or escaped", async () => {
    handler = lists([MINE]);
    show(<WorkspacesPage project="helsinki" />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.workspaces.discard }));
    await screen.findByRole("alertdialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await user.click(screen.getByRole("button", { name: en.workspaces.discard }));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByTestId("confirm-cancel"));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(requests.some((r) => r.method === "DELETE")).toBe(false);
  });

  it("keeps the dialog open with the server's words when discarding is refused, and points to the waiting change on 409", async () => {
    handler = (req, url) => {
      if (req.method === "DELETE") {
        return json({ title: "Conflict", status: 409, detail: "A change of this copy waits for approval." }, 409);
      }
      return lists([MINE])(req, url);
    };
    show(<WorkspacesPage project="helsinki" />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.workspaces.discard }));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByTestId("confirm-accept"));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("A change of this copy waits for approval.");
    expect(within(dialog).getByRole("link", { name: en.workspaces.discardWaiting })).toHaveAttribute("href", "/projects/helsinki/approvals");
    expect(screen.getByText("Air cleanup")).toBeInTheDocument();
    // Closing forgets the refusal: the next opening starts clean.
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await user.click(screen.getByRole("button", { name: en.workspaces.discard }));
    const again = await screen.findByRole("alertdialog");
    expect(within(again).queryByRole("alert")).toBeNull();
  });

  it("shows another refusal without the approvals link", async () => {
    handler = (req, url) =>
      req.method === "DELETE" ? json({ title: "Forbidden", status: 403, detail: "Not your copy." }, 403) : lists([MINE])(req, url);
    show(<WorkspacesPage project="helsinki" />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.workspaces.discard }));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByTestId("confirm-accept"));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Not your copy.");
    expect(within(dialog).queryByRole("link")).toBeNull();
  });

  it("opens the start dialog at /workspaces/new and returns to the list when it closes", async () => {
    handler = lists([]);
    show(<WorkspacesPage project="helsinki" creating />);
    await screen.findByRole("dialog");
    await userEvent.keyboard("{Escape}");
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith({ to: "/projects/$project/workspaces", params: { project: "helsinki" } }),
    );
  });

  it("does not navigate when the start dialog is closed on the list itself", async () => {
    handler = lists([]);
    show(<WorkspacesPage project="helsinki" />);
    const user = userEvent.setup();
    await user.click((await screen.findAllByRole("button", { name: en.workspaces.new }))[0]);
    await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(navigate).not.toHaveBeenCalled();
  });
});
