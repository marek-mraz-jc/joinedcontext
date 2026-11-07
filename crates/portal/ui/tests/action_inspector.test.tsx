/**
 * The Action Inspector (T-0607, AG-56, OPS-50): every tool step opens, a failed one goes back
 * into the run with one click, the trace is copied as JSON, and what the workspace sent is
 * text on the screen and nothing else (AG-46).
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { errorText } from "../src/pages/apps/ActionStep";
import { ConversationPanel } from "../src/pages/apps/ConversationPanel";
import type { RunEvent } from "../src/pages/apps/useAgentRun";
import { expectNoAxeViolations } from "./page_contract";

/** The step rows: each is a `<details>`; the code blocks inside a step are named groups too (T-5014). */
const stepsOf = () => screen.getAllByRole("group").filter((group) => group.tagName === "DETAILS");

const EVENTS: RunEvent[] = [
  {
    seq: 1,
    kind: "tool",
    payload: {
      tool: "search_catalog",
      status: "ok",
      durationMs: 120,
      input: { q: "air quality" },
      output: { items: [{ name: "helsinki-air" }] },
    },
  },
  {
    seq: 2,
    kind: "tool",
    payload: {
      tool: "propose_endpoint",
      status: "failed",
      durationMs: 840,
      input: { name: "air-quality-public" },
      error: "422: representations must name at least one of ngsi-ld, geojson, csv",
      output: "<script>alert(1)</script>",
    },
  },
];

function renderPanel(live = true, events: RunEvent[] = EVENTS) {
  const onSend = vi.fn();
  render(
    <I18nextProvider i18n={i18n}>
      <ConversationPanel
        project="helsinki"
        events={events}
        streaming
        answering={false}
        sending={false}
        live={live}
        onAnswer={() => {}}
        onSend={onSend}
      />
    </I18nextProvider>,
  );
  return onSend;
}

describe("the action inspector", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows every step with its verdict and duration, and opens to the input and the output", async () => {
    renderPanel();
    const steps = stepsOf();
    expect(steps).toHaveLength(2);

    expect(within(steps[0]).getByRole("img", { name: en.agentRun.step.ok })).toBeInTheDocument();
    expect(within(steps[0]).getByText(en.agentRun.step.label.search_catalog)).toBeInTheDocument();
    expect(within(steps[0]).getByText("120 ms")).toBeInTheDocument();

    await userEvent.click(within(steps[0]).getByText(en.agentRun.step.label.search_catalog));
    expect(within(steps[0]).getByText(en.agentRun.step.input)).toBeInTheDocument();
    // T-5516: input and output read as rows, never as JSON text.
    expect(within(steps[0]).getByText("air quality")).toBeInTheDocument();
    expect(within(steps[0]).getByText("helsinki-air")).toBeInTheDocument();
    expect(steps[0].textContent).not.toContain("{\n");
  });

  it("sends a failed step back into the run with the error, only while the run is live", async () => {
    const onSend = renderPanel();
    const failed = stepsOf()[1];
    expect(within(failed).getByRole("img", { name: en.agentRun.step.failed })).toBeInTheDocument();

    await userEvent.click(within(failed).getByText(en.agentRun.step.label.propose_endpoint));
    // T-1031, AG-56: a failed step opens to what it was given as well as why it failed, so an
    // operator reads the arguments that produced the failure instead of guessing them.
    expect(within(failed).getByText(en.agentRun.step.input)).toBeInTheDocument();
    expect(within(failed).getByText(/air-quality-public/)).toBeInTheDocument();
    expect(within(failed).getByText(en.agentRun.step.error)).toBeInTheDocument();

    await userEvent.click(within(failed).getByRole("button", { name: en.agentRun.step.fix }));
    expect(onSend).toHaveBeenCalledTimes(1);
    const text = onSend.mock.calls[0][0] as string;
    // The assistant holds its step's error: the person's own bubble says neither tool nor error.
    expect(text).toContain(en.agentRun.step.label.propose_endpoint);
    expect(text).not.toContain("propose_endpoint");
    expect(text).not.toContain("422");
  });

  it("keeps a failed step's label to one line and says the reason in words on its own row (T-0892, T-6492)", () => {
    renderPanel();
    const reason = screen.getByTestId("step-reason");
    expect(reason).toHaveClass("basis-full");
    expect(reason).not.toHaveClass("truncate");
    expect(reason).toHaveTextContent(en.agentRun.failure.other);
    expect(reason).toHaveAttribute("title", en.agentRun.failure.other);
    const label = reason.parentElement?.querySelector("span.font-sans") as HTMLElement;
    expect(label).not.toBeNull();
    expect(label).toHaveClass("flex-1", "min-w-32");
    expect(label).not.toHaveClass("truncate");
    expect(label).toHaveAttribute("title", label.textContent ?? "");
  });

  // T-2695: the Portal's own step on a handed-over file reads as what it did, never its name.
  it("names the profile of a handed-over file in words", () => {
    renderPanel(true, [
      {
        seq: 1,
        kind: "tool",
        payload: {
          tool: "profile_sample",
          status: "ok",
          durationMs: 1,
          input: { name: "stations.csv", format: "csv" },
          output: { rows: 2, columns: ["station", "bikes"] },
        },
      },
    ]);
    const [step] = stepsOf();
    expect(within(step).getByText(en.agentRun.step.label.profile_sample)).toBeInTheDocument();
    expect(within(step).queryByText("profile_sample")).toBeNull();
  });

  it("shows no duration under a millisecond, and one above it", () => {
    renderPanel(true, [
      { seq: 1, kind: "tool", payload: { tool: "search_catalog", status: "ok", durationMs: 0, input: { q: "bikes" } } },
      { seq: 2, kind: "message", payload: { text: "Found them.", sentBy: "agent" } },
      { seq: 3, kind: "tool", payload: { tool: "query_endpoint", status: "ok", durationMs: 41, input: {} } },
    ]);
    const [search, query] = stepsOf();
    expect(within(search).queryByText(/\bms$/)).toBeNull();
    expect(within(query).getByText("41 ms")).toBeInTheDocument();
  });

  it("offers no fix once the run is over", () => {
    renderPanel(false);
    const failed = stepsOf()[1];
    expect(within(failed).queryByRole("button", { name: en.agentRun.step.fix })).toBeNull();
  });

  it("copies the step as JSON", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    renderPanel();
    const failed = stepsOf()[1];
    await userEvent.click(within(failed).getByText(en.agentRun.step.label.propose_endpoint));
    await userEvent.click(within(failed).getByRole("button", { name: en.agentRun.step.copy }));

    expect(writeText).toHaveBeenCalledTimes(1);
    const trace = JSON.parse(writeText.mock.calls[0][0] as string) as Record<string, unknown>;
    expect(trace.tool).toBe("propose_endpoint");
    expect(trace.status).toBe("failed");
    expect(trace.durationMs).toBe(840);
    expect(await within(failed).findByRole("button", { name: en.agentRun.step.copied })).toBeInTheDocument();
  });

  it("renders what the workspace sent as text, never as markup (AG-46)", async () => {
    renderPanel();
    const failed = stepsOf()[1];
    await userEvent.click(within(failed).getByText(en.agentRun.step.label.propose_endpoint));
    expect(within(failed).getByText("<script>alert(1)</script>")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
  });

  it("says on the step's own row why a function failed, with the file and line", async () => {
    const onSend = renderPanel(true, [
      {
        seq: 7,
        kind: "tool",
        payload: {
          tool: "function:summary",
          status: "failed",
          durationMs: 2,
          error: { file: "@joinedcontext/sdk/server", line: 97, message: "URLSearchParams is not defined" },
          output: { logs: [], status: 500 },
        },
      },
    ]);
    const step = stepsOf()[0];
    const reason = within(step).getByTestId("step-reason");
    expect(reason).toHaveTextContent("URLSearchParams is not defined (@joinedcontext/sdk/server:97)");
    expect(reason).toHaveAttribute("title", "URLSearchParams is not defined (@joinedcontext/sdk/server:97)");

    await userEvent.click(within(step).getByText("function:summary"));
    await userEvent.click(within(step).getByRole("button", { name: en.agentRun.step.fix }));
    const text = onSend.mock.calls[0][0] as string;
    expect(text).toContain("function:summary");
    expect(text).toContain("URLSearchParams is not defined (@joinedcontext/sdk/server:97)");
  });

  it("counts the blocks a patch could not apply and names the first reason", async () => {
    renderPanel(true, [
      {
        seq: 9,
        kind: "tool",
        payload: {
          tool: "apply_patch",
          command: "4 block(s)",
          exitCode: 1,
          applied: [{ path: "src/pages/CustomHtml.tsx", how: "Created" }],
          refused: [
            { path: "src/components/ExportButton.tsx", reason: "SEARCH not found" },
            { path: "src/App.tsx", reason: "SEARCH not found" },
          ],
        },
      },
    ]);
    const step = stepsOf()[0];
    expect(within(step).getByTestId("step-reason")).toHaveTextContent(
      "2 block(s) not applied: src/components/ExportButton.tsx: SEARCH not found",
    );
    await userEvent.click(within(step).getByText("apply_patch"));
    expect(within(step).getByText(en.agentRun.step.refused)).toBeInTheDocument();
  });

  it("shows no reason on a step that succeeded or failed without one", () => {
    const t = (key: string) => key;
    expect(errorText({ status: "ok" }, t)).toBe("");
    expect(errorText({ exitCode: 2 }, t)).toBe("");
    expect(errorText({ error: { message: "timeout" } }, t)).toBe("timeout");
    renderPanel(true, [EVENTS[0]]);
    expect(screen.queryByTestId("step-reason")).toBeNull();
  });

  // T-3986: the app builder draws this card only in a model run, which the live walk cannot start (spend guard).
  it("has_no_serious_axe_violation_as_a_model_run_draws_it", async () => {
    renderPanel();
    expect(stepsOf()).toHaveLength(2);
    await expectNoAxeViolations(document.body);
  });
});
