/**
 * T-2773 (UI-84, AG-87): the assistant on full screen. The owner, 2026-09-24: "when the AI
 * assistant is on full screen it does not look good at all". From 1024 px the person's
 * conversations sit on the left, searchable and a page at a time, the conversation in one
 * centred column, its data and capabilities on the right; each side folds away. Switching
 * between the dock and full screen keeps the conversation and every word typed into it.
 */
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { requestOpen } from "../src/assistant/state";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AssistantDock } from "../src/assistant/AssistantDock";
import { rememberRun } from "../src/assistant/state";
import { CONVERSATIONS_PER_PAGE } from "../src/assistant/ConversationList";
import { expectNoAxeViolations } from "./page_contract";

const PROJECT = "helsinki";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";
const OTHER_RUN = "01J8ZQ4T7K9M2N3P4Q5R6S7T8W";

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
    const frame = new MessageEvent(kind, { data: JSON.stringify(payload), lastEventId: String(payload.seq ?? "") });
    for (const listener of [...(this.listeners.get(kind) ?? [])]) {
      listener(frame);
    }
  }
}

function conversations(count: number) {
  return Array.from({ length: count }, (_, at) => ({
    id: at === 0 ? RUN_ID : at === 1 ? OTHER_RUN : `run-${at}`,
    project: PROJECT,
    kind: "conversation",
    prompt: at === 0 ? "Which stations were empty yesterday?" : at === 1 ? "What are the traffic alerts?" : `Question number ${at}`,
    status: "interviewing",
    createdAt: "2026-09-24T10:00:00Z",
  }));
}

/** A screen at least 1024 px wide, or narrower. */
function screenWide(wide: boolean) {
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: wide,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    })),
  );
}

function renderDock({ layout = "full", runs = 2 }: { layout?: string; runs?: number } = {}) {
  window.sessionStorage.setItem("jc.assistant.layout", layout);
  const read: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url, "http://localhost");
      read.push(`${request.method} ${url.pathname}${url.search}`);
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/auth/me")) {
        return json({ subject: "s", username: "demo.steward", roles: ["domain-editor"] });
      }
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: PROJECT, bootstrap: false, grants: [] });
      }
      const one = /\/agent-runs\/([^/]+)$/.exec(url.pathname);
      if (one) {
        return json({
          id: one[1],
          project: PROJECT,
          kind: "conversation",
          appName: "",
          endpointName: "helsinki-alerts",
          prompt: "Which stations were empty yesterday?",
          status: "interviewing",
          steps: 1,
          tokensUsed: 10,
          createdBy: "demo.steward",
          createdAt: "2026-09-24T10:00:00Z",
          endpoints: [{ name: "helsinki-alerts", slug: "helsinkialertsslug", space: "helsinki" }],
        });
      }
      if (url.pathname.endsWith("/agent-runs")) {
        return json({ items: conversations(runs) });
      }
      if (url.pathname.endsWith("/endpoints")) {
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
      }
      return json({});
    }),
  );
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);
  rememberRun({ project: PROJECT, runId: RUN_ID });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const rootRoute = createRootRoute({ component: () => <AssistantDock project={PROJECT} /> });
  const router = createRouter({ routeTree: rootRoute });
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { read, view };
}

function say(text: string) {
  const source = StubEventSource.opened.at(-1);
  act(() => {
    source?.emit("status", { seq: 1, status: "interviewing" });
    source?.emit("message", { seq: 2, text: "Which stations were empty yesterday?", sentBy: "demo.steward" });
    source?.emit("thought", { seq: 3, text });
  });
}

describe("the assistant on full screen", () => {
  beforeEach(async () => {
    StubEventSource.opened = [];
    window.sessionStorage.clear();
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    rememberRun(null);
  });

  it("lays out the conversations, one centred column and the data beside it", async () => {
    screenWide(true);
    renderDock();
    const list = await screen.findByRole("navigation", { name: en.assistant.full.conversations });
    expect(await within(list).findByRole("button", { name: /Which stations were empty/ })).toHaveAttribute(
      "aria-current",
      "true",
    );
    const column = screen.getByTestId("assistant-column");
    // About 56 rem whatever the screen's width: at 2560 px the sides take the rest.
    expect(column.className).toContain("max-w-4xl");
    expect(column.className).toContain("mx-auto");
    expect(within(column).getByLabelText(en.agentRun.conversation.placeholder)).toBeInTheDocument();
    const data = screen.getByRole("region", { name: en.assistant.full.data });
    expect(within(data).getByRole("group", { name: en.assistant.data.label })).toBeInTheDocument();
    expect(await within(data).findByRole("button", { name: new RegExp(en.assistant.capabilities.button) })).toBeInTheDocument();
    // The data bar is in the side, not repeated above the text box.
    expect(within(column).queryByRole("group", { name: en.assistant.data.label })).toBeNull();
  });

  it("opens a conversation picked from the list in place", async () => {
    screenWide(true);
    const { read } = renderDock();
    const list = await screen.findByRole("navigation", { name: en.assistant.full.conversations });
    await userEvent.click(await within(list).findByRole("button", { name: /traffic alerts/ }));
    await waitFor(() => {
      expect(read).toContain(`GET /api/v1/projects/${PROJECT}/agent-runs/${OTHER_RUN}`);
    });
    expect(within(list).getByRole("button", { name: /traffic alerts/ })).toHaveAttribute("aria-current", "true");
    expect(StubEventSource.opened.at(-1)?.url).toContain(OTHER_RUN);
  });

  it("reads only the person's own conversations, finds by the words asked, and says when none match", async () => {
    screenWide(true);
    const { read } = renderDock();
    const list = await screen.findByRole("navigation", { name: en.assistant.full.conversations });
    await within(list).findByRole("button", { name: /traffic alerts/ });
    expect(read.some((one) => one.includes("/agent-runs?") && one.includes("kind=conversation") && one.includes("mine=true"))).toBe(
      true,
    );
    const search = within(list).getByRole("searchbox", { name: en.assistant.full.search });
    await userEvent.type(search, "TRAFFIC");
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    await userEvent.clear(search);
    await userEvent.type(search, "bikes in 1999");
    expect(within(list).queryAllByRole("listitem")).toHaveLength(0);
    expect(within(list).getByText(/No conversation asks about “bikes in 1999”/)).toBeInTheDocument();
  });

  it("shows the conversations a page at a time", async () => {
    screenWide(true);
    renderDock({ runs: CONVERSATIONS_PER_PAGE + 5 });
    const list = await screen.findByRole("navigation", { name: en.assistant.full.conversations });
    await within(list).findByRole("button", { name: /traffic alerts/ });
    expect(within(list).getAllByRole("listitem")).toHaveLength(CONVERSATIONS_PER_PAGE);
    await userEvent.click(within(list).getByRole("button", { name: "Show 5 more" }));
    expect(within(list).getAllByRole("listitem")).toHaveLength(CONVERSATIONS_PER_PAGE + 5);
    expect(within(list).queryByRole("button", { name: /Show \d+ more/ })).toBeNull();
  });

  it("folds each side away and back, saying which way", async () => {
    screenWide(true);
    renderDock();
    await screen.findByRole("navigation", { name: en.assistant.full.conversations });
    const hideList = screen.getByRole("button", { name: en.assistant.full.hideList });
    expect(hideList).toHaveAttribute("aria-expanded", "true");
    await userEvent.click(hideList);
    expect(screen.queryByRole("navigation", { name: en.assistant.full.conversations })).toBeNull();
    const showList = screen.getByRole("button", { name: en.assistant.full.showList });
    expect(showList).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(screen.getByRole("button", { name: en.assistant.full.hideData }));
    expect(screen.queryByRole("region", { name: en.assistant.full.data })).toBeNull();
    await userEvent.click(showList);
    expect(screen.getByRole("navigation", { name: en.assistant.full.conversations })).toBeInTheDocument();
  });

  it("keeps the conversation and the words typed when the dock goes full screen and back", async () => {
    screenWide(true);
    renderDock({ layout: "side" });
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });
    say("Two stations were empty: **Töölö** and Kamppi.");
    const box = await screen.findByLabelText(en.agentRun.conversation.placeholder);
    await userEvent.type(box, "and today?");
    const before = screen.getByRole("log").textContent;

    await userEvent.click(screen.getByRole("button", { name: en.assistant.fullScreen }));
    expect(await screen.findByTestId("assistant-full")).toBeInTheDocument();
    expect(screen.getByLabelText(en.agentRun.conversation.placeholder)).toHaveValue("and today?");
    expect(screen.getByRole("log").textContent).toBe(before);
    expect(screen.getByText("Töölö").tagName).toBe("STRONG");

    await userEvent.click(screen.getByRole("button", { name: en.assistant.sideView }));
    expect(screen.queryByTestId("assistant-full")).toBeNull();
    expect(screen.getByLabelText(en.agentRun.conversation.placeholder)).toHaveValue("and today?");
  });

  it("is the conversation alone below 1024 px, with the data bar over the text box", async () => {
    screenWide(false);
    renderDock();
    const column = await screen.findByTestId("assistant-column");
    await within(column).findByRole("group", { name: en.assistant.data.label });
    expect(screen.queryByRole("navigation", { name: en.assistant.full.conversations })).toBeNull();
    expect(screen.queryByRole("region", { name: en.assistant.full.data })).toBeNull();
  });

  it("has no axe violation, empty or with a long conversation", async () => {
    screenWide(true);
    const { view } = renderDock();
    await screen.findByRole("button", { name: /traffic alerts/ });
    await waitFor(() => {
      expect(StubEventSource.opened.length).toBeGreaterThan(0);
    });
    say(
      Array.from({ length: 30 }, (_, at) => `- Station ${at}: urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:station-${at}`).join(
        "\n",
      ) + "\n\n| Station | Free |\n|---|---|\n| Töölö | 0 |",
    );
    await screen.findByRole("table");
    await expectNoAxeViolations(view.container);
  });

  // T-2854: below md there is no room for a column beside the page. In the page's row the
  // docked panel squeezed the page to no width and its positioned controls ("Publish this app")
  // painted through the panel at 400 px; below md it covers the screen as full screen does.
  it("docks the app builder, a 40 rem form, only from 2xl, where the page keeps its room (T-3145)", async () => {
    screenWide(false);
    renderDock({ layout: "side" });
    const panel = await screen.findByRole("complementary", { name: en.agentRun.conversation.title });
    act(() => requestOpen("build"));
    await waitFor(() => expect(panel.className.split(/\s+/)).toEqual(expect.arrayContaining(["2xl:sticky", "2xl:w-160"])));
    expect(panel.className).not.toMatch(/(^|\s)(md|lg|xl):sticky/);
  });

  it("docked, covers the screen below xl and is a column beside the page from xl (T-3145)", async () => {
    screenWide(false);
    renderDock({ layout: "side" });
    const panel = await screen.findByRole("complementary", { name: en.agentRun.conversation.title });
    expect(panel).toHaveAttribute("data-layout", "side");
    const classes = panel.className.split(/\s+/);
    expect(classes).toEqual(expect.arrayContaining(["fixed", "inset-x-0", "top-14", "bottom-0", "z-40", "bg-surface"]));
    // Beside the 16 rem sidebar a 24 rem column left a 768 px page 8 rem wide: it docks from xl.
    expect(classes).toEqual(expect.arrayContaining(["xl:sticky", "xl:inset-auto", "xl:z-auto", "xl:shrink-0", "xl:w-96"]));
    expect(classes.some((name) => name.startsWith("md:"))).toBe(false);
    // Nothing unprefixed keeps it in the page's row below xl.
    expect(classes).not.toContain("w-full");
    expect(classes).not.toContain("shrink-0");
  });
});
