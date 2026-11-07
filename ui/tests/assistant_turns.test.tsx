/**
 * T-2461, T-2462, T-2463: a conversation that has answered takes the next question, a stalled
 * one says so with a way on, and an ended or hung one never holds the dock.
 *
 * On dev on 2026-09-21 the owner asked about the alerts, got an answer, and then read "No answer
 * for 55 seconds. The run may still be working, or it may have stopped." under it: the stall
 * clock counted the run's wait for the person as the person's wait for the run. The one button
 * beside it stopped the conversation, after which the panel said it "reads nothing more" and
 * offered nothing, so the owner could neither ask again nor start over.
 */
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { renderPage } from "./page_contract";
import { AssistantDock } from "../src/assistant/AssistantDock";
import { rememberRun } from "../src/assistant/state";
import { ConversationPanel } from "../src/pages/apps/ConversationPanel";
import type { RunEvent } from "../src/pages/apps/useAgentRun";

const PROJECT = "helsinki";
const RUN_ID = "01J8ZQ4T7K9M2N3P4Q5R6S7T8V";
const NEW_RUN = "01J8ZQ4T7K9M2N3P4Q5R6S7T8W";

const event = (seq: number, kind: string, payload: Record<string, unknown>): RunEvent =>
  ({ seq, kind, at: "2026-09-21T05:00:00Z", payload }) as unknown as RunEvent;

/** A question on helsinki-alerts, read and answered: the run now waits for the person. */
const ANSWERED: RunEvent[] = [
  event(1, "status", { status: "interviewing" }),
  event(2, "message", { text: "What alerts are there?", sentBy: "demo.steward" }),
  event(3, "tool", { tool: "query_endpoint", status: "ok", durationMs: 40 }),
  event(4, "thought", { text: "Two situations are open on helsinki-alerts." }),
];

/** The bikes question: a search, then nothing. The agent still owes the answer. */
const STALLED: RunEvent[] = [
  event(1, "status", { status: "interviewing" }),
  event(2, "message", { text: "Which datasets say anything about bikes?", sentBy: "demo.steward" }),
  event(3, "tool", { tool: "search_catalog", status: "ok", durationMs: 0 }),
];

const STOPPED: RunEvent[] = [...STALLED, event(4, "status", { status: "cancelled" })];

function panel(props: {
  events: RunEvent[];
  live?: boolean;
  onSend?: (text: string) => void | Promise<unknown>;
  onCancel?: () => void;
  onRetry?: () => void;
  onNewConversation?: () => void;
}) {
  return (
    <ConversationPanel
      project={PROJECT}
      events={props.events}
      streaming
      answering={false}
      sending={false}
      live={props.live ?? true}
      onAnswer={() => {}}
      onSend={props.onSend ?? (() => {})}
      onCancel={props.onCancel}
      onRetry={props.onRetry}
      onNewConversation={props.onNewConversation}
    />
  );
}

const PAGE = { path: "/projects/helsinki", answer: () => undefined };

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  rememberRun(null);
});

describe("after an answer the person asks the next question (T-2461)", () => {
  it("says_nothing_about_a_stall_while_the_run_waits_for_the_person", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    renderPage(panel({ events: ANSWERED }), PAGE);
    expect(await screen.findByTestId("run-progress")).toHaveTextContent(en.agentRun.progress.waiting);
    await vi.advanceTimersByTimeAsync(120_000);
    expect(screen.queryByText(/No answer for \d+ seconds/)).toBeNull();
    expect(screen.queryByRole("button", { name: en.agentRun.conversation.stop })).toBeNull();
  });

  it("takes_a_second_question_the_moment_the_answer_is_on_screen", async () => {
    const onSend = vi.fn(() => Promise.resolve());
    const user = userEvent.setup();
    renderPage(panel({ events: ANSWERED, onSend }), PAGE);
    const box = await screen.findByLabelText(en.agentRun.conversation.placeholder);
    expect(box).toBeEnabled();
    await user.type(box, "Which of them are road works?");
    const send = screen.getByRole("button", { name: en.agentRun.conversation.send });
    expect(send).toBeEnabled();
    await user.click(send);
    expect(onSend).toHaveBeenCalledWith("Which of them are road works?");
    await waitFor(() => expect((box as HTMLTextAreaElement).value).toBe(""));
  });
});

describe("a run that stops answering (T-2462)", () => {
  it("renders_each_step_as_progress_while_the_answer_is_owed", async () => {
    renderPage(panel({ events: STALLED }), PAGE);
    expect(await screen.findByTestId("run-progress")).toHaveTextContent(en.agentRun.progress.working);
    expect(screen.getByText(en.agentRun.step.label.search_catalog)).toBeInTheDocument();
  });

  it("offers_to_ask_again_and_to_stop_once_the_wait_is_long", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const onRetry = vi.fn();
    const onCancel = vi.fn();
    renderPage(panel({ events: STALLED, onRetry, onCancel }), PAGE);
    await vi.advanceTimersByTimeAsync(50_000);
    expect(await screen.findByText(/No answer for \d+ seconds/)).toBeInTheDocument();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await user.click(screen.getByRole("button", { name: en.agentRun.conversation.retry }));
    expect(onRetry).toHaveBeenCalledOnce();
    await user.click(screen.getByRole("button", { name: en.agentRun.conversation.stop }));
    expect(onCancel).toHaveBeenCalledOnce();
  });
});

describe("an ended run offers a new conversation (T-2463)", () => {
  it("offers_a_new_conversation_where_the_composer_was", async () => {
    const onNewConversation = vi.fn();
    const user = userEvent.setup();
    renderPage(panel({ events: STOPPED, live: false, onNewConversation }), PAGE);
    expect(await screen.findByText(en.agentRun.conversation.closed)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.agentRun.conversation.newConversation }));
    expect(onNewConversation).toHaveBeenCalledOnce();
  });
});

/** The dock over a stub API: the run's record says `status`, the stream says nothing. */
function renderDock(status: string) {
  const calls: { cancelled: number; started: unknown[] } = { cancelled: 0, started: [] };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url, "http://localhost");
      const json = (body: unknown, code = 200) =>
        new Response(JSON.stringify(body), { status: code, headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/auth/me")) {
        return json({ subject: "s", username: "demo.steward", roles: ["domain-editor"] });
      }
      if (url.pathname.endsWith("/permissions/me")) {
        return json({
          project: PROJECT,
          bootstrap: false,
          grants: [{ role: "r", binding: "b", scope: "project", rule: { kinds: ["*"], verbs: ["read", "propose"] } }],
        });
      }
      if (request.method === "POST" && url.pathname.endsWith(`/agent-runs/${RUN_ID}/cancel`)) {
        calls.cancelled += 1;
        return json({ id: RUN_ID, status: "cancelled" });
      }
      if (request.method === "POST" && url.pathname.endsWith("/assistant/conversations")) {
        calls.started.push(JSON.parse(await request.text()));
        return json({ id: NEW_RUN, status: "queued" }, 202);
      }
      if (url.pathname.endsWith(`/agent-runs/${RUN_ID}`)) {
        return json({
          id: RUN_ID,
          project: PROJECT,
          kind: "conversation",
          appName: "",
          endpointName: "",
          prompt: "Which datasets say anything about bikes?",
          status,
          steps: 1,
          tokensUsed: 10,
          createdBy: "demo.steward",
          createdAt: "2026-09-21T05:00:00Z",
          endpoints: [],
        });
      }
      if (url.pathname.endsWith("/endpoints")) {
        return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
      }
      return json({});
    }),
  );
  vi.stubGlobal(
    "EventSource",
    class {
      addEventListener(): void {}
      removeEventListener(): void {}
      close(): void {}
    } as unknown as typeof EventSource,
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <AssistantDock project={PROJECT} /> }),
  });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return calls;
}

describe("the dock's new conversation (T-2463)", () => {
  it.each(["interviewing", "cancelled", "failed"])(
    "starts_a_fresh_conversation_from_a_run_that_is_%s",
    async (status) => {
      const calls = renderDock(status);
      await act(async () => {
        rememberRun({ project: PROJECT, runId: RUN_ID });
      });
      const user = userEvent.setup();
      const fresh = await screen.findByRole("button", { name: en.assistant.newConversation });
      // The run's record has to be read before the dock knows whether the run is over.
      await waitFor(() =>
        expect(
          screen.queryByRole("button", { name: en.assistant.cancel }) === null,
        ).toBe(status !== "interviewing"),
      );
      await user.click(fresh);

      const box = await screen.findByLabelText(en.assistant.empty.composer);
      // A run still going is stopped, not left reading an inbox nobody writes to; an ended one
      // is left alone.
      await waitFor(() => expect(calls.cancelled).toBe(status === "interviewing" ? 1 : 0));
      await user.type(box, "Which datasets say anything about bikes?{Enter}");
      await waitFor(() => expect(calls.started).toHaveLength(1));
      expect(calls.started[0]).toMatchObject({ message: "Which datasets say anything about bikes?" });
    },
  );
});
