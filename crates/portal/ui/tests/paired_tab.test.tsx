// covers (the module gate in gate_modules.test.ts): src/connectors/pairedTab.ts and
// src/connectors/PairedTabBar.tsx, a Portal tab connected to a connector (T-5967).
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { actionOf, runTabAction } from "../src/connectors/pairedTab";
import { PairedTabBar } from "../src/connectors/PairedTabBar";
import { takePrefill } from "../src/assistant/state";

const IDENTITY = { subject: "b7c1e0f4", username: "eva.steward", name: "Eva Steward", roles: [] };
const ID = "abcdefghijklmnopqrstuvwxyz";
const TAB = "0123456789abcdef0123456789abcdef";
const CONNECTOR = {
  id: ID,
  name: "doprava-in-claude",
  owner: IDENTITY.subject,
  segments: ["pipeline"],
  scope: [{ project: "doprava", access: "read" }],
  writes: false,
  expiresAt: new Date(Date.now() + 10 * 24 * 3600_000).toISOString(),
  allowedClients: ["mcp-local"],
  clientIsBoundary: false,
  limits: { parallelCalls: 4, resultBytes: 1048576 },
  state: "active",
  createdAt: "2026-10-28T12:00:00Z",
  lastUsedAt: null,
  url: `https://portal.dev.example/c/${ID}`,
};

/** The browser's EventSource, driven by the test: one instance per stream the bar opens. */
class FakeSource {
  static CLOSED = 2;
  static opened: FakeSource[] = [];
  readyState = 1;
  onerror: (() => void) | null = null;
  listeners = new Map<string, (event: MessageEvent<string>) => void>();
  constructor(public url: string) {
    FakeSource.opened.push(this);
  }
  addEventListener(kind: string, listener: (event: MessageEvent<string>) => void) {
    this.listeners.set(kind, listener);
  }
  removeEventListener(kind: string) {
    this.listeners.delete(kind);
  }
  close() {
    this.readyState = FakeSource.CLOSED;
  }
  send(data: unknown) {
    this.listeners.get("action")?.(new MessageEvent("action", { data: JSON.stringify(data) }));
  }
}

function renderAt(path: string) {
  const requests: { method: string; path: string }[] = [];
  vi.stubGlobal("EventSource", FakeSource);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, code = 200) =>
        Promise.resolve(new Response(code === 204 ? null : JSON.stringify(body), { status: code, headers: { "Content-Type": "application/json" } }));
      if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
      if (url.pathname.endsWith("/branding")) return json({ instanceName: "joinedcontext", domain: "dev.example", languages: { default: "en", offered: ["en"] } });
      if (url.pathname.endsWith("/permissions/me")) return json({ project: "doprava", bootstrap: false, grants: [] });
      if (url.pathname === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: "doprava" }] });
      if (url.pathname.startsWith("/api/v1/connectors")) {
        requests.push({ method: request.method, path: url.pathname });
        if (url.pathname === `/api/v1/connectors/${ID}/tab` && request.method === "POST") {
          return json({ tab: TAB, connector: ID, name: CONNECTOR.name, until: "2026-10-29T12:00:00Z" }, 201);
        }
        if (url.pathname === `/api/v1/connectors/${ID}/tab` && request.method === "DELETE") return json(null, 204);
        if (url.pathname === "/api/v1/connectors") return json({ items: [CONNECTOR] });
      }
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  window.history.pushState({}, "", path);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { requests };
}

const ACTION = {
  frame: "navigate",
  tool: "jc_ui_open_form",
  project: "doprava",
  payload: { route: "/projects/doprava/pipelines/new", prefill: { metadata: { name: "hel-news" }, spec: { compute: { kind: "bloblang", bloblang: "root = this" } } } },
  from: { connector: "doprava-in-claude", client: "mcp-local" },
  at: "2026-10-29T03:00:00Z",
};

describe("an action for a connected tab (T-5967)", () => {
  it("runs only a Portal route or a known ui action, and hands a form its values once", async () => {
    expect(actionOf({ ...ACTION, payload: { route: "https://evil.example/" } })).toBeNull();
    expect(actionOf({ ...ACTION, frame: "submit" })).toBeNull();
    expect(actionOf("text")).toBeNull();
    const go = vi.fn();
    const entry = await runTabAction(ACTION, go);
    expect(go).toHaveBeenCalledWith("/projects/doprava/pipelines/new");
    expect(entry).toMatchObject({ tool: "jc_ui_open_form", client: "mcp-local", ran: true });
    expect(takePrefill("/projects/doprava/pipelines/new")).toMatchObject({ metadata: { name: "hel-news" } });
    // A filter with no table on the page says it did not run, rather than pretending.
    const missed = await runTabAction({ ...ACTION, frame: "ui", tool: "jc_ui_filter_table", payload: { action: "filter", query: "bikes" } }, go);
    expect(missed?.ran).toBe(false);
  }, 10_000);
});

describe("Connect this tab (T-5967)", () => {
  beforeEach(async () => {
    FakeSource.opened = [];
    sessionStorage.clear();
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("pairs the tab, runs what the connector sends with who sent it, and disconnects", async () => {
    const user = userEvent.setup();
    const { requests } = renderAt("/ai-connectors");
    expect(await screen.findByText(/Connect this tab to let the AI client/)).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "Connect this tab to doprava-in-claude" }));
    // The shell of every page mounts the bar, not My connectors alone.
    expect(PairedTabBar.name).toBe("PairedTabBar");
    const bar = await screen.findByRole("region", { name: "Tab connected to a connector" });
    expect(within(bar).getByText("Connected to doprava-in-claude")).toBeInTheDocument();
    expect(within(bar).getByText(/cannot save, propose or approve/)).toBeInTheDocument();
    await waitFor(() => expect(FakeSource.opened.at(-1)?.url).toBe(`/api/v1/connectors/${ID}/tab/${TAB}/events`));

    act(() => FakeSource.opened.at(-1)?.send(ACTION));
    await waitFor(() => expect(window.location.pathname).toBe("/projects/doprava/pipelines/new"));
    const live = await screen.findByRole("region", { name: "Tab connected to a connector" });
    expect(within(live).getByText("Last: Opened a form: /projects/doprava/pipelines/new")).toBeInTheDocument();
    await user.click(within(live).getByRole("button", { name: "Actions (1)" }));
    expect(within(live).getByText(/^Claude Code.* via doprava-in-claude$/)).toBeInTheDocument();
    // Nothing the connector sent wrote anything: the only writes are the person's two clicks.
    await user.click(within(live).getByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Tab connected to a connector" })).toBeNull());
    expect(requests.filter((r) => r.method !== "GET")).toEqual([
      { method: "POST", path: `/api/v1/connectors/${ID}/tab` },
      { method: "DELETE", path: `/api/v1/connectors/${ID}/tab` },
    ]);
  }, 20_000);

  it("says so when the server ends the pairing", async () => {
    sessionStorage.setItem("jc.pairedTab", JSON.stringify({ connector: ID, name: CONNECTOR.name, tab: TAB }));
    renderAt("/ai-connectors");
    await waitFor(() => expect(FakeSource.opened.length).toBeGreaterThan(0));
    const source = FakeSource.opened.at(-1);
    act(() => {
      source?.close();
      source?.onerror?.();
    });
    expect(await screen.findByText(/This tab is no longer connected/)).toBeInTheDocument();
    expect(sessionStorage.getItem("jc.pairedTab")).toBeNull();
  }, 20_000);

  it("reads the open page again when the connector's client made a call, and runs no action for it", async () => {
    sessionStorage.setItem("jc.pairedTab", JSON.stringify({ connector: ID, name: CONNECTOR.name, tab: TAB }));
    const { requests } = renderAt("/ai-connectors");
    await waitFor(() => expect(FakeSource.opened.length).toBeGreaterThan(0));
    const reads = () => requests.filter((r) => r.path === "/api/v1/connectors").length;
    await waitFor(() => expect(reads()).toBeGreaterThan(0), { timeout: 10_000 });
    const before = reads();
    const source = FakeSource.opened.at(-1);
    act(() => {
      source?.send({ frame: "refresh", payload: {}, tool: "jc_change_plan" });
      source?.send({ frame: "refresh", payload: {}, tool: "jc_change_get" });
    });
    await waitFor(() => expect(reads()).toBe(before + 1), { timeout: 5_000 });
    expect(screen.queryByText(/jc_change_plan/)).not.toBeInTheDocument();
  }, 20_000);

  it("keeps the pairing while the page unloads: the browser closes the stream, the next page listens", async () => {
    sessionStorage.setItem("jc.pairedTab", JSON.stringify({ connector: ID, name: CONNECTOR.name, tab: "tab-of-the-reload" }));
    renderAt("/ai-connectors");
    await waitFor(() => expect(FakeSource.opened.length).toBeGreaterThan(0));
    const source = FakeSource.opened.at(-1);
    source?.close();
    source?.onerror?.();
    expect(sessionStorage.getItem("jc.pairedTab")).not.toBeNull();
    expect(screen.queryByText(/This tab is no longer connected/)).not.toBeInTheDocument();
  }, 20_000);
});
