/**
 * T-1763, T-1764: a request that failed is not "you have nothing" (UI-01, TS-19).
 *
 * Both panels on the Assistant page consumed their query data-only — `access.isError` threw the
 * server's own sentence away and `access.isPending` was never read at all, and `runsQuery.isError`
 * was never checked, so a failure fell through to the empty state. A person whose request 403'd
 * or 500'd was told, confidently and wrongly, that the agent held no access and that their
 * conversations and unattended runs were gone, with no reason and nothing to press.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { answeringChecks } from "./checks";

const PROJECT = "helsinki";

/** The sentence the API puts in a problem detail; the panel must show this and not swallow it. */
const REFUSED = "you may not read this project's agent access";

interface Failing {
  /** Which path fails, and with what. Everything else answers normally. */
  path: "/assistant/access" | "/agent-runs" | "/schema/index.json";
  status: number;
  detail: string;
}

let attempts = 0;

function renderPage(failing: Failing | null, healAfterFirst = false, runs: unknown[] = []) {
  attempts = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      const path = new URL(request.url, "http://localhost").pathname;
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        });
      if (path.endsWith("/auth/me")) return json({ subject: "s", username: "reader", roles: [], groups: [PROJECT] });
      if (path.endsWith("/projects")) return json([PROJECT]);

      const fails =
        failing !== null &&
        (failing.path === "/agent-runs"
          ? path.endsWith("/agent-runs") && request.method === "GET"
          : failing.path === "/schema/index.json"
            ? path.endsWith("/schema/index.json")
            : path.endsWith("/assistant/access"));
      if (fails) {
        attempts += 1;
        if (!(healAfterFirst && attempts > 1)) {
          return json(
            { type: "about:blank", title: "Refused", status: failing.status, detail: failing.detail },
            failing.status,
          );
        }
      }

      if (path.endsWith("/assistant/access")) return json({ items: [] });
      if (path.endsWith("/agent-runs") && request.method === "GET") return json({ items: runs });
      if (path.includes("/endpoints")) {
        return json({
          items: [
            {
              apiVersion: "joinedcontext.com/v1alpha1",
              kind: "Endpoint",
              metadata: { name: "ovzdusie", namespace: PROJECT },
              spec: { slug: "abc123" },
            },
          ],
        });
      }
      if (path.endsWith("/schema/index.json")) {
        return json({ $defs: { AirQualityObserved: { properties: { pm10: { type: "number" } } } } });
      }
      return json({ items: [] });
    }),
  );
  vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
  vi.stubGlobal("EventSource", class {
    addEventListener() {}
    removeEventListener() {}
    close() {}
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("a list that failed says so on the Assistant page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", `/projects/${PROJECT}/assistant`);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("a_refused_agent_access_list_shows_the_servers_own_reason_not_you_have_none", async () => {
    renderPage({ path: "/assistant/access", status: 403, detail: REFUSED });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(REFUSED);
    expect(
      screen.queryByText(en.assistantPage.access.none),
      "a refusal is not an empty list",
    ).not.toBeInTheDocument();
  });

  it("and_the_person_can_try_the_agent_access_list_again", async () => {
    renderPage({ path: "/assistant/access", status: 500, detail: "the database is away" }, true);

    const retry = await screen.findByRole("button", { name: en.form.listRetry });
    await userEvent.click(retry);

    // The second answer succeeds and is empty, so the empty state is what is right to show now.
    await waitFor(() => expect(screen.getByText(en.assistantPage.access.none)).toBeInTheDocument());
    expect(screen.queryByRole("button", { name: en.form.listRetry })).not.toBeInTheDocument();
    expect(attempts, "the retry really asked again").toBeGreaterThan(1);
  });

  it("a_failed_run_list_is_never_drawn_as_the_empty_state", async () => {
    // `EmptyState` renders `role="status"`; the failure must not reach it.
    renderPage({ path: "/agent-runs", status: 500, detail: "the run store is away" });

    await waitFor(() =>
      expect(screen.getAllByRole("alert").some((node) => node.textContent?.includes("the run store is away"))).toBe(true),
    );
    expect(
      screen.queryByText(en.assistantPage.empty),
      "their conversations are not gone, the request failed",
    ).not.toBeInTheDocument();
  });

  it("a_genuinely_empty_agent_access_list_still_says_so", async () => {
    renderPage(null);
    expect(await screen.findByText(en.assistantPage.access.none)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.form.listRetry })).not.toBeInTheDocument();
  });

  it("a_run_that_failed_says_why_and_not_only_that_it_did", async () => {
    renderPage(null, false, [
      {
        id: "run-1",
        project: PROJECT,
        kind: "application",
        appName: "ovzdusie",
        status: "failed",
        prompt: "build the air quality page",
        createdAt: "2026-09-20T08:00:00Z",
        error: "the runner refused: no egress to registry.npmjs.org",
      },
    ]);
    expect(
      await screen.findByText("the runner refused: no egress to registry.npmjs.org"),
    ).toBeInTheDocument();
  });

  it("the_mine_filter_is_the_shared_checkbox_with_the_portals_own_focus_ring", async () => {
    renderPage(null);
    const mine = await screen.findByRole("checkbox", { name: en.assistantPage.filters.mine });
    expect(mine.className, "the hand-made ring was focus:ring-*, not the shared utility").not.toMatch(
      /focus:ring-/,
    );
    expect(mine.closest("label")?.className).toMatch(/cursor-pointer/);
  });
});
