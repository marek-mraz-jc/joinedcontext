/**
 * The assistant drives the Portal (T-0609, UI-45, AG-45, AG-46).
 *
 * What is worth asserting is what a person cannot see from the chat: that a `navigate` frame
 * really moves the router and opens the form with the values, that the dock survives the move,
 * and that a frame naming anything but a path inside the Portal moves nothing.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { isPortalRoute, rememberPrefill, settlePrefill, takePrefill } from "../src/assistant/state";
import { expectDenied } from "./checks";
import { findFormPage } from "./formPage";

const PROJECT = "banskabystrica";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";

const RUN = {
  id: RUN_ID,
  project: PROJECT,
  appName: "ovzdusie-dnes",
  endpointName: "ovzdusie-public",
  appClass: "static",
  visibility: "project",
  prompt: "A map of the stations with today's PM10",
  status: "building",
  steps: 3,
  tokensUsed: 1_200,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-12T08:00:00Z",
};

const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", roles: ["domain-editor"] };

const SPACES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: { name: "ovzdusie", project: PROJECT },
      spec: { tenant: "ovzdusie" },
    },
  ],
};

class StubEventSource {
  static opened: StubEventSource[] = [];
  readonly url: string;
  private readonly listeners = new Map<string, Set<EventListener>>();

  constructor(url: string) {
    this.url = url;
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
    const frame = new MessageEvent(kind, {
      data: JSON.stringify(payload),
      lastEventId: String(payload.seq ?? ""),
    });
    for (const listener of [...(this.listeners.get(kind) ?? [])]) {
      listener(frame);
    }
  }
}

let requests: Request[] = [];
const fetchCalls = () => requests;

/** A path's button is named by its title and its line (T-2692). */
const pathName = (path: keyof typeof en.assistant.paths) =>
  `${en.assistant.paths[path].title} ${en.assistant.paths[path].line}`;

function renderPortal(permissions?: unknown) {
  requests = [];
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    requests.push(request);
    const path = new URL(request.url, "http://localhost").pathname;
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      );
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/permissions/me") && permissions) {
      return json(permissions);
    }
    if (path.endsWith(`/agent-runs/${RUN_ID}`)) {
      return json(RUN);
    }
    if (path.endsWith("/agent-runs/new-conv-run-id")) {
      return json({
        ...RUN,
        id: "new-conv-run-id",
        appName: "",
        kind: "conversation",
        status: "interviewing",
        prompt: "Which datasets say anything about bikes?",
      });
    }
    if (path.endsWith("/assistant/conversations") && request.method === "POST") {
      return json(
        {
          ...RUN,
          id: "new-conv-run-id",
          appName: "",
          kind: "conversation",
          status: "interviewing",
        },
        202,
      );
    }
    if (path.endsWith("/agent-runs") && request.method === "GET") {
      return json({ items: [] });
    }
    if (path.endsWith("/spaces")) {
      return json(SPACES);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

/** The dock's stream: every dock opens one, and the run page's own comes first. */
const dockStream = () => StubEventSource.opened.at(-1) as StubEventSource;

async function emitToEveryStream(kind: string, payload: Record<string, unknown>) {
  await act(async () => {
    for (const stream of StubEventSource.opened) {
      stream.emit(kind, payload);
    }
  });
}

describe("the route rule", () => {
  it("lets only a path inside the Portal through", () => {
    expect(isPortalRoute("/projects/helsinki/endpoints?tab=all")).toBe(true);
    expect(isPortalRoute("/")).toBe(true);
    for (const bad of [
      "https://evil.example/",
      "//evil.example/projects",
      "javascript:alert(1)",
      "projects/helsinki",
      "/projects/helsinki#x",
      "/projects/hel\nsinki",
      `/${"a".repeat(512)}`,
      "",
      42,
      null,
    ]) {
      expect(isPortalRoute(bad), String(bad)).toBe(false);
    }
  });

  it("hands a prefill to its own path, and to it again while the person is there (T-0907)", () => {
    rememberPrefill("/projects/x/endpoints?tab=all", { name: "air" });
    expect(takePrefill("/projects/x/pipelines")).toBeNull();
    expect(takePrefill("/projects/x/endpoints")).toEqual({ name: "air" });
    // The page mounts twice for one hand-off: the address settles behind the route change and
    // `HandOff` remounts it. The second mount is the one the person sees.
    expect(takePrefill("/projects/x/endpoints")).toEqual({ name: "air" });
    settlePrefill("/projects/x/pipelines");
    expect(takePrefill("/projects/x/endpoints")).toBeNull();
  });

  it("hands a prefill storage refuses, too: the tab keeps it until the page takes it (T-0891)", () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("quota", "QuotaExceededError");
    });
    try {
      rememberPrefill("/projects/x/spaces/complete?space=bikes", { result: { space: "bikes", drafts: [] } });
    } finally {
      setItem.mockRestore();
    }
    expect(takePrefill("/projects/x/spaces/complete")).toEqual({ result: { space: "bikes", drafts: [] } });
    expect(takePrefill("/projects/x/spaces/complete")).toEqual({ result: { space: "bikes", drafts: [] } });
    settlePrefill("/projects/x/models");
    expect(takePrefill("/projects/x/spaces/complete")).toBeNull();
  });
});

describe("the assistant dock", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    StubEventSource.opened = [];
    window.sessionStorage.clear();
    window.history.pushState({}, "", `/projects/${PROJECT}/apps?run=${RUN_ID}`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("follows a navigate frame to the endpoints page and opens the form prefilled", async () => {
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });

    await emitToEveryStream("navigate", {
      seq: 7,
      route: `/projects/${PROJECT}/endpoints`,
      prefill: { name: "air-quality-public", contextSpaceRef: "ovzdusie" },
    });

    await waitFor(() => {
      expect(window.location.pathname).toBe(`/projects/${PROJECT}/endpoints`);
    });
    const dialog = await findFormPage();
    await waitFor(() => {
      expect((dialog.querySelector('input[name="name"], #root_name') as HTMLInputElement).value).toBe(
        "air-quality-public",
      );
    });
    // The dock came along, and says what it did.
    const notice = await screen.findByText("Opened Endpoints");
    expect(notice).toBeInTheDocument();
    // UI-58, T-0958: the form says the assistant filled it, so the person reads the values as
    // a proposal to check rather than as something they typed.
    expect(within(dialog).getByText(en.assistant.prefilledNotice)).toBeInTheDocument();
    expect(dockStream().url).toBe(`/api/v1/projects/${PROJECT}/agent-runs/${RUN_ID}/events`);
    // And it is where it always is: the column on the right of the page, after the main
    // content, on this page as on the run page.
    const dock = notice.closest("aside") as HTMLElement;
    // Docked: a column beside the page from xl up (2xl for the builder); below it covers the screen (T-2854, T-3145).
    expect(dock.className).toMatch(/\b2?xl:sticky\b/);
    expect(dock.className).toContain("border-l");
    const main = document.querySelector("main") as HTMLElement;
    expect(main.compareDocumentPosition(dock) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("opens Complete this space with the drafts the agent handed over (T-0907)", async () => {
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });

    // The person is on the page they asked from — the data sources of the project, which is
    // where the integrate story starts. That page reads a hand-off of its own as it mounts, and
    // the address moves before the route does: on dev it took the drafts meant for Complete
    // this space and the page opened empty (T-0907).
    await act(async () => {
      window.history.pushState({}, "", `/projects/${PROJECT}/datasources`);
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    await waitFor(() => {
      expect(window.location.pathname).toBe(`/projects/${PROJECT}/datasources`);
    });

    // What `jc_space_complete` sends when the agent has drafted a space: the route carries the
    // space it made, the prefill the drafts it checked (AG-73).
    await emitToEveryStream("navigate", {
      seq: 7,
      route: `/projects/${PROJECT}/spaces/complete?space=city-bikes`,
      prefill: {
        url: "https://example.com/free_bike_status.json",
        result: {
          space: "city-bikes",
          found: [],
          drafts: [
            {
              kind: "Pipeline",
              name: "city-bikes-load",
              inferred: true,
              manifest: { kind: "Pipeline", metadata: { name: "city-bikes-load" } },
              verdict: { ok: true, findings: [], inputDigest: "abc" },
            },
          ],
          proposeReady: true,
          lane: "yellow",
          change: null,
        },
      },
    });

    await waitFor(() => {
      expect(window.location.pathname).toBe(`/projects/${PROJECT}/spaces/complete`);
    });
    // The page is mounted more than once for this one hand-off; the drafts are on the one the
    // person sees, or the page reads as if the agent had done nothing.
    expect(await screen.findByTestId("complete-draft-Pipeline")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Propose all/i })).toBeEnabled();
  });

  it("never replays a navigate it followed once the person has moved on to another page", async () => {
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });
    const frame = { seq: 7, route: `/projects/${PROJECT}/endpoints`, prefill: { name: "air-quality-public" } };
    await emitToEveryStream("navigate", frame);
    await waitFor(() => {
      expect(window.location.pathname).toBe(`/projects/${PROJECT}/endpoints`);
    });
    expect(await screen.findByText("Opened Endpoints")).toBeInTheDocument();

    // The person opens the change for review; the dock on that page replays the run's events,
    // and the notice about the page they left is gone.
    await act(async () => {
      window.history.pushState({}, "", `/projects/${PROJECT}/approvals`);
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    await waitFor(() => {
      expect(window.location.pathname).toBe(`/projects/${PROJECT}/approvals`);
    });
    await emitToEveryStream("navigate", frame);
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(window.location.pathname).toBe(`/projects/${PROJECT}/approvals`);
    expect(screen.queryByText("Opened Endpoints")).not.toBeInTheDocument();

    // A navigate the run sends later still leads the way.
    await emitToEveryStream("navigate", { seq: 9, route: `/projects/${PROJECT}/spaces` });
    await waitFor(() => {
      expect(window.location.pathname).toBe(`/projects/${PROJECT}/spaces`);
    });
  });

  it("opens the grant it drafts on the access page the person is already on (T-0770)", async () => {
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    await act(async () => {
      window.history.pushState({}, "", `/projects/${PROJECT}/access`);
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    // The page's own button; an empty list offers the same first grant beside it (T-3246).
    await waitFor(() => expect(screen.getAllByRole("button", { name: en.access.roles.grant }).length).toBeGreaterThan(0));
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });

    const name = "jana-kovacova-steward-banskabystrica";
    await emitToEveryStream("navigate", {
      seq: 11,
      route: `/projects/${PROJECT}/access?grant=${name}`,
      draft: { kind: "RoleBinding", name },
      prefill: {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "RoleBinding",
        metadata: { name, project: "org" },
        spec: { subjects: [{ user: "jana.kovacova" }], roleRef: "steward", scope: { project: PROJECT } },
      },
    });

    const dialog = await findFormPage(en.access.roles.grantTitle);
    expect(within(dialog).getByLabelText(new RegExp(en.access.roles.personLabel))).toHaveValue("jana.kovacova");
  });

  it("sends what the person types to the application run it follows, not to a new conversation (T-0739)", async () => {
    const user = userEvent.setup();
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    const dock = (await screen.findByRole("button", { name: en.assistant.hide })).closest("aside") as HTMLElement;
    expect(within(dock).getByTestId("assistant-app")).toHaveTextContent("Ovzdusie dnes");

    await user.type(within(dock).getByLabelText(en.agentRun.conversation.placeholder), "Make the map bigger");
    await user.click(within(dock).getByRole("button", { name: en.agentRun.conversation.send }));

    await waitFor(() => {
      expect(
        fetchCalls().some(
          (request) => request.method === "POST" && request.url.endsWith(`/agent-runs/${RUN_ID}/messages`),
        ),
      ).toBe(true);
    });
    const sent = fetchCalls().find((request) => request.url.endsWith(`/agent-runs/${RUN_ID}/messages`)) as Request;
    expect(await sent.clone().json()).toEqual({ text: "Make the map bigger" });
    expect(fetchCalls().some((request) => request.url.endsWith("/assistant/conversations"))).toBe(false);
  });

  // UI-52: hide to the bubble, full screen, stop the run (POST …/cancel) and close.
  it("hides to a bubble at the bottom right, opens full screen, stops the run and closes", async () => {
    const user = userEvent.setup();
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    const dock = (await screen.findByRole("button", { name: en.assistant.hide })).closest(
      "aside",
    ) as HTMLElement;
    // The dock says which application the run builds, by name read as words.
    expect(within(dock).getByTestId("assistant-app")).toHaveTextContent("Ovzdusie dnes");
    expect(within(dock).getByTestId("assistant-app")).toHaveAttribute("title", RUN.appName);

    await user.click(within(dock).getByRole("button", { name: en.assistant.fullScreen }));
    expect(dock.className).toContain("fixed");
    expect(dock.className).not.toMatch(/\b2?xl:sticky\b/);
    await user.click(within(dock).getByRole("button", { name: en.assistant.sideView }));
    // Docked: a column beside the page from xl up (2xl for the builder); below it covers the screen (T-2854, T-3145).
    expect(dock.className).toMatch(/\b2?xl:sticky\b/);

    await user.click(within(dock).getByRole("button", { name: en.assistant.hide }));
    const bubble = screen.getByRole("button", { name: en.assistant.open });
    expect(bubble.className).toContain("fixed");
    expect(bubble.className).toContain("right-4");
    expect(bubble.className).toContain("bottom-4");
    await user.click(bubble);
    const reopened = screen.getByRole("button", { name: en.assistant.hide }).closest("aside") as HTMLElement;

    await user.click(within(reopened).getByRole("button", { name: en.assistant.cancel }));
    await waitFor(() => {
      expect(
        fetchCalls().some(
          (request) =>
            request.method === "POST" &&
            request.url.endsWith(`/agent-runs/${RUN_ID}/cancel`),
        ),
      ).toBe(true);
    });

    await user.click(within(reopened).getByRole("button", { name: en.assistant.close }));
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: en.assistant.hide })).not.toBeInTheDocument();
    });
    // Closing forgets the run; the bubble stays so the person can always ask again (UI-51).
    expect(screen.getByRole("button", { name: en.assistant.open })).toBeInTheDocument();
    expect(window.sessionStorage.getItem("jc.assistant.run")).toBeNull();
  });

  it("floats over the page, keeps that layout for the tab, and attaches a file from the composer", async () => {
    const user = userEvent.setup();
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    const dock = (await screen.findByRole("button", { name: en.assistant.hide })).closest(
      "aside",
    ) as HTMLElement;

    expect(within(dock).getByRole("button", { name: en.assistant.sideView })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await user.click(within(dock).getByRole("button", { name: en.assistant.floatView }));
    expect(dock.dataset.layout).toBe("float");
    expect(dock.className).toContain("fixed");
    expect(dock.className).not.toMatch(/\b2?xl:sticky\b/);
    // The theme's elevation token, not a Tailwind default (T-1749).
    expect(dock.className).toContain("shadow-3");
    // On a phone the floating panel takes the screen under the header (T-4204).
    expect(dock.className).toContain("max-sm:inset-x-0");
    expect(dock.className).toContain("max-sm:top-14");
    expect(dock.className).toContain("max-sm:max-w-none");
    expect(within(dock).getByRole("button", { name: en.assistant.floatView })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(window.sessionStorage.getItem("jc.assistant.layout")).toBe("float");

    // The sample file is a paperclip in the composer, not a box of text under the chat.
    expect(within(dock).getByLabelText(en.assistant.attach)).toHaveAttribute("type", "file");
    expect(within(dock).queryByText(/Drop a CSV/)).not.toBeInTheDocument();
  });

  it("ignores a navigate frame that is not a path inside the Portal", async () => {
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });
    const before = window.location.pathname;

    await emitToEveryStream("navigate", { seq: 8, route: "https://evil.example/", prefill: {} });
    await emitToEveryStream("navigate", { seq: 9, route: "//evil.example/x" });

    expect(window.location.pathname).toBe(before);
    expect(screen.queryByText(/opened/)).not.toBeInTheDocument();
  });

  it("renders bubble on page without a run, opens empty state and starts conversation on a path click", async () => {
    window.sessionStorage.clear();
    window.history.pushState({}, "", `/projects/${PROJECT}/spaces`);
    renderPortal();

    const bubble = await screen.findByRole("button", { name: en.assistant.open });
    expect(bubble).toBeInTheDocument();

    const user = userEvent.setup();
    await user.click(bubble);

    const emptyState = await screen.findByTestId("assistant-empty");
    expect(emptyState).toBeInTheDocument();
    expect(screen.getByText(i18n.t("assistant.empty.lead"))).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: pathName("find-data") }));

    await waitFor(() => {
      expect(
        fetchCalls().some(
          (req) =>
            req.method === "POST" &&
            req.url.includes(`/projects/${PROJECT}/assistant/conversations`),
        ),
      ).toBe(true);
    });

    await waitFor(() => {
      expect(screen.queryByTestId("assistant-empty")).toBeNull();
    });
    expect(screen.getByRole("heading", { name: en.assistant.title })).toBeInTheDocument();
  });

  it("keeps the paths a viewer cannot take, disabled with the reason (T-1390, T-2692)", async () => {
    window.history.pushState({}, "", `/projects/${PROJECT}/spaces`);
    renderPortal({
      project: PROJECT,
      bootstrap: false,
      grants: [{ role: "viewer", binding: "v", rule: { kinds: ["*"], verbs: ["get", "list"] } }],
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.assistant.open }));
    await screen.findByTestId("assistant-empty");

    expect(screen.getByRole("button", { name: pathName("find-data") })).toBeEnabled();
    for (const [path, kind] of [["share-data", "Endpoint"], ["build-dashboard", "Dashboard"], ["build-app", "App"]] as const) {
      await waitFor(() =>
        expect(screen.getByRole("button", { name: pathName(path) })).toHaveAttribute("aria-disabled", "true"),
      );
      expectDenied(
        screen.getByRole("button", { name: pathName(path) }),
        i18n.t("permissions.denied", { verb: "propose", kind }),
      );
    }
    // A disabled path starts nothing.
    await user.click(screen.getByRole("button", { name: pathName("build-dashboard") }));
    expect(fetchCalls().some((req) => req.method === "POST")).toBe(false);
  });

  // UI-52: Escape returns a full-screen panel to its place beside the page.
  it("leaves full screen on Escape key", async () => {
    const user = userEvent.setup();
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    const dock = (await screen.findByRole("button", { name: en.assistant.hide })).closest(
      "aside",
    ) as HTMLElement;

    await user.click(within(dock).getByRole("button", { name: en.assistant.fullScreen }));
    expect(dock.className).toContain("fixed");
    expect(dock.className).not.toMatch(/\b2?xl:sticky\b/);

    await user.keyboard("{Escape}");
    await waitFor(() => {
      // Docked: a column beside the page from xl up (2xl for the builder); below it covers the screen (T-2854, T-3145).
      expect(dock.className).toMatch(/\b2?xl:sticky\b/);
    });
  });

  // UI-52: the hidden panel's bubble shows an activity dot while the assistant works.
  it("shows busy dot on bubble while newest event is a person message", async () => {
    renderPortal();
    await screen.findByRole("heading", { name: "Ovzdusie dnes" });
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });

    const dock = (await screen.findByRole("button", { name: en.assistant.hide })).closest(
      "aside",
    ) as HTMLElement;
    const user = userEvent.setup();
    await user.click(within(dock).getByRole("button", { name: en.assistant.hide }));

    const bubble = screen.getByRole("button", { name: en.assistant.open });
    expect(within(bubble).queryByTestId("assistant-busy")).toBeNull();

    await emitToEveryStream("message", {
      seq: 2,
      kind: "message",
      payload: { text: "Can you change the colors?", sentBy: "jana.kovacova" },
    });

    await waitFor(() => {
      expect(within(bubble).getByTestId("assistant-busy")).toBeInTheDocument();
    });
    // A screen reader hears what the dot shows (T-6230).
    expect(bubble).toHaveAccessibleName(en.assistant.openBusy);

    await emitToEveryStream("thought", {
      seq: 3,
      kind: "thought",
      payload: { text: "Adjusting color scheme..." },
    });

    await waitFor(() => {
      expect(within(bubble).queryByTestId("assistant-busy")).toBeNull();
    });
    expect(bubble).toHaveAccessibleName(en.assistant.open);
  });
});
