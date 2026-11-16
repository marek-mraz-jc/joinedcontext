/**
 * T-2424, T-2423, T-2421: where the assistant's input sits, what stands beside it, and which
 * corners it has.
 *
 * Three defects the user met on 2026-09-20, all in the same dock. The composer sat in the middle of
 * a scrolling column with a `Recent conversations` list of truncated prompts under it, so the input
 * moved as content grew and the last thing in the panel was a list of old questions. The paths
 * (T-2692) are the empty state's and only its (T-2464 reversed T-2423, which had
 * put them inside a conversation too). And every corner in the dock was Tailwind's bare `rounded`,
 * 0.25rem, which is on no token of `src/tokens.css`, beside a page built from `rounded-md` and
 * `rounded-lg`.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { expectDenied } from "./checks";
import en from "../src/locales/en.json";
import { AssistantDock } from "../src/assistant/AssistantDock";
import { rememberRun } from "../src/assistant/state";

const PROJECT = "helsinki";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";
const OTHER_RUN = "01J8ZQ4T7K9M2N3P4Q5R6S7T8W";

/** Two live conversations, which is what used to draw the `Resume:` list. */
const RECENT = {
  items: [
    { id: RUN_ID, project: PROJECT, kind: "conversation", prompt: "Which stations were empty yesterday?", status: "interviewing" },
    { id: OTHER_RUN, project: PROJECT, kind: "conversation", prompt: "what are currenlt helsinky traffic alerts ??", status: "answering" },
  ],
};

class StubEventSource {
  static opened: StubEventSource[] = [];
  private readonly listeners = new Map<string, Set<EventListener>>();
  constructor() {
    StubEventSource.opened.push(this);
  }
  addEventListener(kind: string, listener: EventListener): void {
    const set = this.listeners.get(kind) ?? new Set<EventListener>();
    set.add(listener);
    this.listeners.set(kind, set);
  }
  removeEventListener(kind: string, listener: EventListener): void {
    this.listeners.get(kind)?.delete(listener);
  }
  close(): void {}
  emit(kind: string, payload: Record<string, unknown>): void {
    const frame = new MessageEvent(kind, { data: JSON.stringify(payload), lastEventId: String(payload.seq ?? "") });
    for (const listener of [...(this.listeners.get(kind) ?? [])]) {
      listener(frame);
    }
  }
}

/** The rule the caller holds: `propose` on everything, or read only; `all` proposes every path's kind. */
function permissions(mayPropose: boolean, all = false) {
  return {
    project: PROJECT,
    bootstrap: false,
    grants: [
      {
        role: "reader",
        binding: "readers",
        scope: "project",
        rule: {
          kinds: all ? ["Endpoint", "Dashboard", "Pipeline", "ContextSpace", "App", "DataModel"] : ["Endpoint", "Dashboard"],
          verbs: mayPropose ? ["read", "propose"] : ["read"],
        },
      },
    ],
  };
}

function renderDock({
  mayPropose = true,
  proposesAll = false,
  hiddenSections = [],
}: { mayPropose?: boolean; proposesAll?: boolean; hiddenSections?: string[] } = {}) {
  const messages: unknown[] = [];
  const started: unknown[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = input as Request;
    const url = new URL(request.url, "http://localhost");
    const json = (body: unknown, status = 200) =>
      new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "s", username: "demo.steward", roles: ["domain-editor"] });
    }
    if (url.pathname.endsWith("/permissions/me")) {
      return json(permissions(mayPropose, proposesAll));
    }
    if (url.pathname === "/api/v1/branding") {
      return json({ hiddenSections });
    }
    if (request.method === "POST" && url.pathname.endsWith("/assistant/conversations")) {
      started.push(JSON.parse(await request.text()));
      return json({ id: RUN_ID, status: "queued" }, 202);
    }
    if (request.method === "POST" && url.pathname.endsWith(`/agent-runs/${RUN_ID}/messages`)) {
      messages.push(JSON.parse(await request.text()));
      return json({}, 202);
    }
    if (url.pathname.endsWith(`/agent-runs/${RUN_ID}`)) {
      return json({
        id: RUN_ID,
        project: PROJECT,
        kind: "conversation",
        appName: "",
        endpointName: "helsinki-alerts",
        prompt: "what are currenlt helsinky traffic alerts ??",
        status: "interviewing",
        steps: 1,
        tokensUsed: 10,
        createdBy: "demo.steward",
        createdAt: "2026-09-20T17:00:00Z",
        endpoints: [{ name: "helsinki-alerts", slug: "helsinkialertsslug", space: "helsinki" }],
      });
    }
    if (url.pathname.endsWith("/agent-runs")) {
      return json(RECENT);
    }
    if (url.pathname.endsWith("/endpoints")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }
    return json({});
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const rootRoute = createRootRoute({ component: () => <AssistantDock project={PROJECT} /> });
  const router = createRouter({ routeTree: rootRoute });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { messages, started };
}

/** The paths of ADR-N-032 in the order the dock lists them, and the kind each proposes. */
const PATH_IDS = [
  "integrate-pipeline",
  "upload-data",
  "find-data",
  "share-data",
  "build-app",
  "build-dashboard",
  "create-data-model",
  "define-kpi",
] as const;
type PathId = (typeof PATH_IDS)[number];
const PROPOSES: [PathId, string][] = [
  ["integrate-pipeline", "Pipeline"],
  ["upload-data", "ContextSpace"],
  ["share-data", "Endpoint"],
  ["build-app", "App"],
  ["build-dashboard", "Dashboard"],
  ["create-data-model", "DataModel"],
  ["define-kpi", "Pipeline"],
];
/** A path's button is named by its title and its line. */
const pathName = (path: PathId) =>
  `${en.assistant.paths[path].title} ${en.assistant.paths[path].line}`;

const dock = () => screen.getByRole("complementary", { name: en.agentRun.conversation.title });

/** The dock opens as a bubble; a person clicks it. */
async function openDock() {
  const person = userEvent.setup();
  await person.click(await screen.findByRole("button", { name: en.assistant.open }));
  return person;
}

/**
 * The panel's own children, in the order a person reads them: a `contents` wrapper draws no box
 * of its own (T-2773 keeps one tree for every layout), so its children stand in its place.
 */
const laidOut = (element: Element): HTMLElement[] =>
  ([...element.children] as HTMLElement[]).flatMap((child) =>
    child.classList.contains("contents") ? laidOut(child) : [child],
  );
const children = () => laidOut(dock());

describe("where the assistant's input sits", () => {
  beforeEach(async () => {
    StubEventSource.opened = [];
    rememberRun(null);
    window.localStorage.clear();
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
    rememberRun(null);
  });

  it("puts the text box last in the empty state, with only scrolling content above it", async () => {
    renderDock();
    await openDock();
    const composer = await screen.findByLabelText(en.assistant.empty.composer);
    const form = composer.closest("form") as HTMLElement;
    const last = children().at(-1);
    expect(last, "the composer is the panel's last child, so it cannot be pushed down").toBe(form);
    // What is above it scrolls; the form itself does not.
    expect(screen.getByTestId("assistant-empty").className).toContain("overflow-y-auto");
    expect(form.className).not.toContain("overflow-y-auto");
  });

  it("draws no list of old prompts under the text box, with two live conversations to draw", async () => {
    renderDock();
    await openDock();
    await screen.findByLabelText(en.assistant.empty.composer);
    for (const prompt of RECENT.items) {
      expect(screen.queryByText(new RegExp(prompt.prompt.slice(0, 20), "i"))).toBeNull();
    }
    expect(screen.queryByRole("button", { name: /resume/i })).toBeNull();
    // and the two strings that drew it are gone from the bundle, in every language
    const empty = en.assistant.empty as Record<string, unknown>;
    expect(empty.recent).toBeUndefined();
    expect(empty.resume).toBeUndefined();
  });

  // T-2464: the owner, in the live demo — "this should not be there.... when having conversation".
  it("shows the paths once in a new conversation and never inside one", async () => {
    renderDock();
    await openDock();
    const paths = await screen.findByTestId("assistant-paths");
    expect(within(paths).getAllByRole("button")).toHaveLength(PATH_IDS.length);
    expect(screen.getAllByRole("button", { name: pathName("find-data") })).toHaveLength(1);

    await act(async () => {
      rememberRun({ project: PROJECT, runId: RUN_ID });
    });
    const composer = await screen.findByLabelText(en.agentRun.conversation.placeholder);
    expect(screen.queryByTestId("assistant-paths")).toBeNull();
    expect(screen.queryByTestId("assistant-suggestions")).toBeNull();
    for (const path of PATH_IDS) {
      expect(screen.queryByRole("button", { name: pathName(path) })).toBeNull();
    }
    // Nothing stands between the thread and the composer but the data bar: no chip, no initials.
    const form = composer.closest("form") as HTMLElement;
    const above = (form.firstElementChild as HTMLElement).textContent ?? "";
    expect(above.startsWith(en.assistant.data.title), above).toBe(true);
  });

  it("offers the same paths again under New conversation", async () => {
    renderDock();
    const person = await openDock();
    await act(async () => {
      rememberRun({ project: PROJECT, runId: RUN_ID });
    });
    await screen.findByLabelText(en.agentRun.conversation.placeholder);
    await person.click(screen.getByRole("button", { name: en.assistant.newConversation }));
    const paths = await screen.findByTestId("assistant-paths");
    expect(within(paths).getAllByRole("button").map((button) => button.dataset.path)).toEqual(PATH_IDS);
  });

  // T-2874: an installation that hides dashboards does not offer their path; showing them brings it back.
  it("leaves out the path of a section the installation hides", async () => {
    renderDock({ hiddenSections: ["dashboards"] });
    await openDock();
    const paths = await screen.findByTestId("assistant-paths");
    await waitFor(() =>
      expect(within(paths).getAllByRole("button").map((button) => button.dataset.path)).toEqual(
        PATH_IDS.filter((path) => path !== "build-dashboard"),
      ),
    );
  });

  // T-2692, AG-91: a path starts with nothing typed; the Portal asks its first question itself.
  it("starts a conversation on the path a person clicks, with no prompt", async () => {
    const { started } = renderDock();
    const person = await openDock();
    const share = await screen.findByRole("button", { name: pathName("share-data") });
    await waitFor(() => expect(share).not.toHaveAttribute("aria-disabled"));
    await person.click(share);
    await waitFor(() => expect(started).toHaveLength(1));
    expect(started[0]).toMatchObject({ message: "", path: "share-data" });
    // The conversation is open: the paths are gone.
    await waitFor(() => expect(screen.queryByTestId("assistant-paths")).toBeNull());
  });

  // T-2763: the run is told the page the person started it from.
  it("sends the page the person is on with the conversation it starts", async () => {
    window.history.pushState({}, "", `/projects/${PROJECT}/spaces/helsinki?tab=inside`);
    const { started } = renderDock();
    const person = await openDock();
    const share = await screen.findByRole("button", { name: pathName("share-data") });
    await waitFor(() => expect(share).not.toHaveAttribute("aria-disabled"));
    await person.click(share);
    await waitFor(() => expect(started).toHaveLength(1));
    expect(started[0]).toMatchObject({ pageContext: { route: `/projects/${PROJECT}/spaces/helsinki?tab=inside` } });
    window.history.pushState({}, "", "/");
  });

  it("disables every path the caller's role cannot take, with the reason on it", async () => {
    const { started } = renderDock({ mayPropose: true });
    const person = await openDock();
    const paths = await screen.findByTestId("assistant-paths");
    for (const [path, kind] of PROPOSES.filter(([, kind]) => kind !== "Endpoint" && kind !== "Dashboard")) {
      // The reason is `permissions.denied` filled in by i18next, which is what a person reads.
      await waitFor(() =>
        expect(within(paths).getByRole("button", { name: pathName(path) })).toHaveAttribute("aria-disabled", "true"),
      );
      expectDenied(
        within(paths).getByRole("button", { name: pathName(path) }),
        i18n.t("permissions.denied", { verb: "propose", kind }),
      );
    }
    // A refused path starts nothing.
    await person.click(within(paths).getByRole("button", { name: pathName("build-app") }));
    expect(started).toHaveLength(0);
    // The reason is also on screen, once, not only in a tooltip (T-4263).
    expect(screen.getByTestId("assistant-paths-denied")).toHaveTextContent(en.assistant.pathsDenied);
  });

  it("lets an editor without an App grant ask: Find data and the composer stay open (T-6464)", async () => {
    const { started } = renderDock({ mayPropose: true });
    const person = await openDock();
    const paths = await screen.findByTestId("assistant-paths");
    await waitFor(() =>
      expect(within(paths).getByRole("button", { name: pathName("build-app") })).toHaveAttribute("aria-disabled", "true"),
    );
    expect(screen.queryByTestId("assistant-read-only")).toBeNull();
    await person.click(within(paths).getByRole("button", { name: pathName("find-data") }));
    await waitFor(() => expect(started).toHaveLength(1));
    expect(started[0]).toMatchObject({ path: "find-data" });
  });

  it("tells a read-only role before they type, and starts nothing (T-6464)", async () => {
    const { started } = renderDock({ mayPropose: false });
    const person = await openDock();
    expect(await screen.findByTestId("assistant-read-only")).toHaveTextContent(en.assistant.readOnly);
    const paths = screen.getByTestId("assistant-paths");
    const find = within(paths).getByRole("button", { name: pathName("find-data") });
    expect(find).toHaveAttribute("aria-disabled", "true");
    expectDenied(find, en.assistant.readOnly);
    await person.click(find);
    expect(screen.getByRole("textbox", { name: en.assistant.empty.composer })).toBeDisabled();
    expect(screen.queryByTestId("assistant-paths-denied")).toBeNull();
    expect(started).toHaveLength(0);
  });

  it("says nothing about greyed paths to a role that may take them all", async () => {
    renderDock({ proposesAll: true });
    await openDock();
    const paths = await screen.findByTestId("assistant-paths");
    await waitFor(() =>
      expect(within(paths).getByRole("button", { name: pathName("build-app") })).not.toHaveAttribute("aria-disabled"),
    );
    expect(screen.queryByTestId("assistant-paths-denied")).toBeNull();
  });

  it("reaches the paths from the keyboard in the order they are listed", async () => {
    renderDock({ mayPropose: false });
    const person = await openDock();
    const paths = await screen.findByTestId("assistant-paths");
    await waitFor(() =>
      expect(within(paths).getByRole("button", { name: pathName("build-app") })).toHaveAttribute("aria-disabled", "true"),
    );
    within(paths).getByRole("button", { name: pathName("integrate-pipeline") }).focus();
    const reached = [(document.activeElement as HTMLElement).dataset.path];
    for (let step = 1; step < PATH_IDS.length; step += 1) {
      await person.tab();
      reached.push((document.activeElement as HTMLElement).dataset.path);
    }
    // A refused path keeps its place, so a person who cannot see the grey hears why.
    expect(reached).toEqual(PATH_IDS);
  });
});

describe("the assistant's corners", () => {
  const here = dirname(fileURLToPath(import.meta.url));

  it("come from the radius scale, never Tailwind's bare rounded", () => {
    for (const file of ["AssistantDock.tsx", "EndpointPicker.tsx", "HandOff.tsx"]) {
      const source = readFileSync(resolve(here, join("..", "src", "assistant", file)), "utf8");
      const bare = [...source.matchAll(/\brounded(?=[ "'`])/g)];
      expect(bare, `${file} uses rounded-sm|md|lg|xl|full, not the bare class`).toEqual([]);
    }
  });
});
