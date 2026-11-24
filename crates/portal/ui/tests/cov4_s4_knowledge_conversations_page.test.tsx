// T-7706: ConversationsPage uncovered paths: a hidden project, a turn without steps, the same-question
// read failing or empty, and a refused clipboard on the compare view's copy.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { InRouter } from "./pageHarness";
import { ConversationsPage } from "../src/pages/knowledge/ConversationsPage";

const PROJECT = "helsinki";
const ONE = "11111111-1111-4111-8111-111111111111";
const TWO = "22222222-2222-4222-8222-222222222222";
const ROW = {
  id: ONE, deployment: "obcan", origin: null, startedAt: "2026-10-18T10:00:00Z", lastAt: "2026-10-18T10:01:00Z",
  firstQuestion: "Aké je PM10?", turns: 1, routes: ["data"], totalMs: 2400, errors: 0, timeouts: 0,
  costUsd: 0.0046, uncosted: 0, thumbsUp: 0, thumbsDown: 0, tokensIn: 10, tokensOut: 5, cachedTokens: null,
  firstTokenMs: 800, models: [],
};
const TURN = {
  turn: 1, at: "2026-10-18T10:01:00Z", question: "Aké je PM10?", answer: "PM10 je 31.", route: "data",
  firstTokenMs: 800, totalMs: 2400, error: null, timedOut: false, costUsd: 0.0046, uncosted: 0, verdict: null, steps: [],
};

type SameMode = "error" | "empty" | "other";
let sameMode: SameMode = "other";

function stub(permissionsStatus: number) {
  const calls: URL[] = [];
  vi.stubGlobal("fetch", (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init);
    const url = new URL(request.url, "http://localhost");
    calls.push(url);
    const reply = (status: number, body: unknown) =>
      Promise.resolve(new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
      }));
    if (url.pathname.endsWith("/permissions/me")) {
      if (permissionsStatus === 404) return reply(404, { title: "Not Found", status: 404, detail: "project 'helsinki' not found" });
      return reply(200, {
        bootstrap: false, project: PROJECT,
        grants: [{ role: "r", binding: "b", scope: PROJECT, space: null, rule: { kinds: ["AssistantDeployment"], verbs: ["read", "approve"] } }],
      });
    }
    if (url.pathname.endsWith("/same")) {
      if (sameMode === "error") return reply(500, { title: "Internal", status: 500, detail: "same read exploded" });
      if (sameMode === "empty") return reply(200, { items: [{ conversation: ONE, turn: 1, at: TURN.at, deployment: "obcan", totalMs: 1, costUsd: 0, steps: 0 }] });
      return reply(200, { items: [{ conversation: TWO, turn: 1, at: TURN.at, deployment: "obcan", totalMs: 3100, costUsd: 0.006, steps: 0 }] });
    }
    if (url.pathname.endsWith(TWO)) return reply(200, { id: TWO, deployment: "obcan", origin: null, turns: [TURN] });
    if (url.pathname.endsWith(ONE)) return reply(200, { id: ONE, deployment: "obcan", origin: null, turns: [TURN] });
    if (url.pathname.endsWith("/conversations/usage")) return reply(404, { title: "n", status: 404, detail: "none" });
    if (url.pathname.endsWith("/assistantdeployments")) return reply(200, { items: [] });
    return reply(200, { items: [ROW], nextCursor: null });
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter><ConversationsPage project={PROJECT} /></InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return calls;
}

afterEach(() => {
  sameMode = "other";
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("ConversationsPage edges (T-7706)", () => {
  it("hides the whole page behind the project-hidden notice when permissions answer 404", async () => {
    const calls = stub(404);
    expect(await screen.findByText(i18n.t("form.projectHidden"))).toBeInTheDocument();
    expect(calls.some((c) => c.pathname.endsWith("/conversations"))).toBe(false);
  });

  it("opens a turn that has no steps without a step timeline", async () => {
    const user = userEvent.setup();
    stub(200);
    await user.click(await screen.findByRole("link", { name: "Aké je PM10?" }));
    expect(await screen.findByText("PM10 je 31.")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: i18n.t("conversations.steps") })).toBeNull();
  });

  it("shows the problem when the same-question read fails", async () => {
    sameMode = "error";
    const user = userEvent.setup();
    stub(200);
    await user.click(await screen.findByRole("link", { name: "Aké je PM10?" }));
    await user.click(await screen.findByRole("button", { name: i18n.t("conversations.same") }));
    expect(await screen.findByText(/same read exploded/)).toBeInTheDocument();
  });

  it("says no other answer exists when only the turn itself comes back", async () => {
    sameMode = "empty";
    const user = userEvent.setup();
    stub(200);
    await user.click(await screen.findByRole("link", { name: "Aké je PM10?" }));
    await user.click(await screen.findByRole("button", { name: i18n.t("conversations.same") }));
    expect(await screen.findByText(i18n.t("conversations.sameEmpty"))).toBeInTheDocument();
  });

  it("does not claim Copied when the clipboard refuses the pair", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(() => Promise.reject(new Error("denied")));
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    stub(200);
    await user.click(await screen.findByRole("link", { name: "Aké je PM10?" }));
    await user.click(await screen.findByRole("button", { name: i18n.t("conversations.same") }));
    const same = await screen.findByRole("list", { name: i18n.t("conversations.same") });
    await user.click(within(same).getByRole("button", { name: i18n.t("conversations.compare") }));
    const view = await screen.findByRole("region", { name: i18n.t("conversations.compareTitle") });
    await user.click(await within(view).findByRole("button", { name: i18n.t("conversations.copyPair") }));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(within(view).queryByText(i18n.t("conversations.copied"))).toBeNull();
  });
});
