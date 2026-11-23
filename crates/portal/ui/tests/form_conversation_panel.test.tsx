/**
 * T-1761: the conversation's composer and what the panel says when the run goes quiet
 * (UI-04, UI-15, UI-16, UI-39, UI-48).
 *
 * Four defects, all of them about a person being told nothing. The box emptied itself the moment
 * send fired and the dock never rendered the mutation's error, so a failed message vanished with
 * no word and nothing left to press send on again — the assistant looked like it was ignoring
 * them. A stream that died mid-answer changed one grey caption from "live" to "offline" while
 * half-written answers stayed on screen looking finished. An answer that never arrived was
 * indistinguishable from one still being written, with no stall notice and no way to stop. And
 * an empty conversation was a bare grey line.
 */
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { checkForm, type FormSpec } from "./formContract";
import { LOCALES, renderPage } from "./page_contract";
import { ConversationPanel } from "../src/pages/apps/ConversationPanel";
import type { RunEvent } from "../src/pages/apps/useAgentRun";

const EVENTS: RunEvent[] = [
  { seq: 1, kind: "message", at: "2026-09-20T09:00:00Z", payload: { role: "user", text: "Build a bike dashboard" } },
  { seq: 2, kind: "message", at: "2026-09-20T09:00:04Z", payload: { role: "assistant", text: "Looking at the endpoint." } },
] as unknown as RunEvent[];

function panel(props: Partial<{
  events: RunEvent[];
  streaming: boolean;
  answering: boolean;
  sending: boolean;
  onSend: (text: string) => void | Promise<unknown>;
  onCancel: () => void;
}> = {}) {
  return (
    <ConversationPanel
      project="helsinki"
      events={props.events ?? EVENTS}
      streaming={props.streaming ?? true}
      answering={props.answering ?? false}
      sending={props.sending ?? false}
      live
      onAnswer={() => {}}
      onSend={props.onSend ?? (() => {})}
      onCancel={props.onCancel}
    />
  );
}

const spec: FormSpec = {
  fields: [{ id: "run-message", label: /Tell the assistant/i, value: "Add the station names" }],
  submit: /^Send$/,
  path: "/projects/helsinki",
  answer: () => undefined,
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("the composer (T-1761)", () => {
  it("meets_the_form_contract", async () => {
    await checkForm((proposed) => panel({ onSend: () => proposed() }), spec);
  });

  it("keeps_every_word_and_says_why_when_the_message_does_not_leave", async () => {
    const user = userEvent.setup();
    renderPage(panel({ onSend: () => Promise.reject(new Error("The run is not accepting messages.")) }), {
      path: "/projects/helsinki",
      answer: () => undefined,
    });
    const box = await screen.findByLabelText(/Tell the assistant/i);
    await user.type(box, "Add the station names");
    await user.click(screen.getByRole("button", { name: /^Send$/ }));

    expect(await screen.findByText(/The run is not accepting messages\./)).toBeInTheDocument();
    expect((box as HTMLTextAreaElement).value, "the message was lost with the send").toBe(
      "Add the station names",
    );
  });

  it("says_why_try_again_did_not_leave_and_keeps_the_offer", async () => {
    const user = userEvent.setup();
    const events = [
      { seq: 1, kind: "message", payload: { text: "how many bikes are free?", sentBy: "jana" } },
      { seq: 2, kind: "thought", payload: { text: "The answer failed: no answer in time.", failed: true } },
    ] as unknown as RunEvent[];
    const onSend = vi
      .fn<(text: string) => Promise<unknown>>()
      .mockRejectedValueOnce(new Error("The run is not accepting messages."))
      .mockRejectedValueOnce("offline");
    renderPage(panel({ events, onSend }), { path: "/projects/helsinki", answer: () => undefined });
    const again = await screen.findByRole("button", { name: en.agentRun.conversation.tryAgain });
    await user.click(again);
    expect(await screen.findByText(/The run is not accepting messages\./)).toBeInTheDocument();
    expect(onSend).toHaveBeenLastCalledWith("how many bikes are free?");

    // A refusal that is no Error says the Portal's own sentence instead of the reason.
    await user.click(screen.getByRole("button", { name: en.agentRun.conversation.tryAgain }));
    expect(await screen.findByText(new RegExp(en.app.error.generic.replace(/[.?]/g, "\\$&")))).toBeInTheDocument();
    expect(onSend).toHaveBeenCalledTimes(2);
  });

  it("sends_nothing_while_the_last_message_is_still_leaving", async () => {
    const user = userEvent.setup();
    const onSend = vi.fn();
    renderPage(panel({ sending: true, onSend }), { path: "/projects/helsinki", answer: () => undefined });
    const box = await screen.findByLabelText(/Tell the assistant/i);
    await user.type(box, "Add the station names{Enter}");
    expect(onSend).not.toHaveBeenCalled();
    expect((box as HTMLTextAreaElement).value).toBe("Add the station names");
  });

  it("empties_the_box_once_the_message_has_left", async () => {
    const user = userEvent.setup();
    renderPage(panel({ onSend: () => Promise.resolve({ ok: true }) }), {
      path: "/projects/helsinki",
      answer: () => undefined,
    });
    const box = await screen.findByLabelText(/Tell the assistant/i);
    await user.type(box, "Add the station names");
    await user.click(screen.getByRole("button", { name: /^Send$/ }));
    await waitFor(() => expect((box as HTMLTextAreaElement).value).toBe(""));
  });
});

describe("what the panel says when the run goes quiet (T-1761)", () => {
  it("says_the_live_connection_dropped_instead_of_one_grey_word", async () => {
    renderPage(panel({ streaming: false }), { path: "/projects/helsinki", answer: () => undefined });
    expect(await screen.findByText(i18n.t("agentRun.conversation.dropped"))).toBeInTheDocument();
  });

  it("says_how_long_it_has_been_waiting_and_offers_to_stop_the_run", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const stop = vi.fn();
    renderPage(panel({ answering: true, onCancel: stop }), {
      path: "/projects/helsinki",
      answer: () => undefined,
    });
    await vi.advanceTimersByTimeAsync(50_000);
    const notice = await screen.findByText(/No answer for \d+ seconds/);
    expect(notice).toBeInTheDocument();
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await user.click(screen.getByRole("button", { name: i18n.t("agentRun.conversation.stop") }));
    expect(stop).toHaveBeenCalledOnce();
  });

  it("says_what_a_new_conversation_is_for_instead_of_one_grey_line", async () => {
    renderPage(panel({ events: [] }), { path: "/projects/helsinki", answer: () => undefined });
    expect(await screen.findByText(i18n.t("agentRun.conversation.empty"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("agentRun.conversation.emptyHint"))).toBeInTheDocument();
  });

  it.each(LOCALES)("says_the_quiet_run_in_%s", async (locale) => {
    await i18n.changeLanguage(locale);
    renderPage(panel({ streaming: false }), { path: "/projects/helsinki", answer: () => undefined });
    expect(await screen.findByText(i18n.t("agentRun.conversation.dropped"))).toBeInTheDocument();
  });
});
