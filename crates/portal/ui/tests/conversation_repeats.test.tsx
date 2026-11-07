/**
 * A step the run repeats in a row with the same outcome is drawn once, with a count: a preview
 * that calls a failing function on every render must not bury the conversation.
 */
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ConversationPanel, foldRepeats } from "../src/pages/apps/ConversationPanel";
import type { RunEvent } from "../src/pages/apps/useAgentRun";

const ERROR = { file: "@joinedcontext/sdk/server", line: 97, message: "URLSearchParams is not defined" };

function failure(seq: number, error: unknown = ERROR): RunEvent {
  return {
    seq,
    kind: "tool",
    payload: {
      tool: "function:summary",
      status: "failed",
      durationMs: seq,
      input: { types: ["BikeHireDockingStation"] },
      output: { logs: [], status: 500 },
      error,
    },
  };
}

function panel(events: RunEvent[]) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ConversationPanel
        project="helsinki"
        events={events}
        streaming
        answering={false}
        sending={false}
        live
        onAnswer={() => {}}
        onSend={() => {}}
      />
    </I18nextProvider>,
  );
}

describe("repeated steps", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("folds five identical failures into one step with the count, keeping the newest", () => {
    const folded = foldRepeats([1, 2, 3, 4, 5].map((seq) => failure(seq)));
    expect(folded).toHaveLength(1);
    expect(folded[0]).toMatchObject({ count: 5, event: { seq: 5 } });

    panel([1, 2, 3, 4, 5].map((seq) => failure(seq)));
    expect(screen.getAllByText("function:summary")).toHaveLength(1);
    expect(screen.getByTestId("step-count")).toHaveTextContent("×5");
    expect(screen.getAllByRole("button", { name: en.agentRun.step.copy })).toHaveLength(1);
  });

  it("keeps two different errors as two steps", () => {
    panel([failure(1), failure(2, { message: "timeout" })]);
    expect(screen.getAllByText("function:summary")).toHaveLength(2);
    expect(screen.queryByTestId("step-count")).toBeNull();
  });

  it("starts a new step when something else came between the repeats", () => {
    const message: RunEvent = {
      seq: 3,
      kind: "message",
      payload: { text: "add more kpis on the overview", sentBy: "demo.steward@hel.fi" },
    };
    const folded = foldRepeats([failure(1), failure(2), message, failure(4)]);
    expect(folded.map(({ event, count }) => [event.seq, count])).toEqual([
      [2, 2],
      [3, 1],
      [4, 1],
    ]);
  });
});
