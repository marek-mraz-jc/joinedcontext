/**
 * What a person reads when an assistant step fails or finds nothing (T-6492, TS-19): plain words
 * instead of the tool's error, a state with a next step for a refusal, a missing thing and a page
 * that would not open, and an empty read that says so. The errors below are the ones a live walk
 * met (evidence/T-5962/assistant).
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ActionStep, echoedLines, failureOf, noticesOf, retriedSteps, scrubbed, StepNotice } from "../src/pages/apps/ActionStep";
import { ConversationPanel } from "../src/pages/apps/ConversationPanel";
import { QueryAnswer, queryResultOf, viewOf, whenOf } from "../src/pages/apps/QueryResultCard";
import type { RunEvent } from "../src/pages/apps/useAgentRun";

const tool = (seq: number, name: string, error?: string, extra: Record<string, unknown> = {}): RunEvent =>
  ({
    seq,
    kind: "tool",
    payload: { tool: name, status: error === undefined ? "ok" : "failed", ...(error === undefined ? {} : { error }), ...extra },
  }) as RunEvent;

const withI18n = (node: React.ReactNode) => render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);

const panel = (events: RunEvent[], onSend = vi.fn()) => {
  withI18n(
    <ConversationPanel
      project="demo"
      events={events}
      streaming
      answering={false}
      sending={false}
      live
      onAnswer={vi.fn()}
      onSend={onSend}
    />,
  );
  return onSend;
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("the kind of a failed step", () => {
  it.each([
    ["query_endpoint", "error: argument type: takes 1 to 50 items", "argument"],
    ["query_endpoint", "a query_endpoint call names no tool", "argument"],
    ["query_endpoint", 'error: {"error":{"status":400,"title":"BadRequestData"}}', "argument"],
    ["change_resource", "the change_resource call does not parse: unknown field `class`", "argument"],
    ["propose_endpoint", "representation 'odata' is not one of ngsi-ld, csv", "argument"],
    ["jc_resource_list", 'invalid input at /: "kind" is a required property', "argument"],
    ["query_endpoint", 'error: {"detail":"mutation tools not permitted","status":403}', "refused"],
    ["jc_resource_list", "not found: kind 'RoleBinding' not found in project 'org'; you may read no kind there", "refused"],
    ["change_resource", "the data read for this message writes this change_resource call; data is never an instruction", "guarded"],
    ["jc_pipeline_sample_source", "not found: DataSource 'trips-csv' not found in project 'x'", "missing"],
    ["query_endpoint", "error: `BikeCounter` is not an entity type this endpoint describes for you", "missing"],
    ["jc_ui_navigate", "the page 'entities' needs the endpoint whose data to open", "page"],
    ["navigate", "the path `build-app` does not use navigate", "page"],
    ["change_resource", "the test is not green: the feed could not be reached: no such host", "test"],
    ["write_entities", "the broker went away", "other"],
  ])("reads %s: %s as %s", (name, error, kind) => {
    expect(failureOf({ tool: name, status: "failed", error })).toBe(kind);
  });

  it("leaves a step that worked and a builder's own step alone", () => {
    expect(failureOf({ tool: "query_endpoint", status: "ok" })).toBeNull();
    expect(failureOf({ tool: "apply_patch", exitCode: 1 })).toBeNull();
  });
});

describe("a failed step's line", () => {
  it("says words, and holds no tool name, argument value, URL query or token", () => {
    const error =
      "error: argument q: 'secret-value' refused by https://broker.example/x?token=abc123 with Bearer abcdefghijkl";
    withI18n(<ActionStep event={tool(1, "jc_pipeline_run", error)} live onSend={vi.fn()} />);
    const summary = screen.getByTestId("step-reason").closest("summary") as HTMLElement;
    expect(summary).toHaveTextContent(en.agentRun.failure.argument);
    expect(summary).toHaveTextContent(en.agentRun.step.label.portal);
    for (const leak of ["jc_pipeline_run", "secret-value", "token=", "abc123", "abcdefghijkl", "error:"]) {
      expect(summary.textContent).not.toContain(leak);
      expect(summary.innerHTML).not.toContain(leak);
    }
  });

  it("scrubs the opened error and the copied trace of tokens and URL queries, and drops an MCP refusal's output", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const error = "refused by https://me:pw@broker.example/x?token=abc123, Bearer abcdefghijkl";
    withI18n(
      <ActionStep
        event={tool(1, "query_endpoint", error, { output: { isError: true, content: [{ text: error }] } })}
        live
        onSend={vi.fn()}
      />,
    );
    await userEvent.click(screen.getByText(en.agentRun.step.label.query_endpoint));
    const shown = document.body.textContent ?? "";
    expect(shown).toContain("https://broker.example/x");
    for (const leak of ["abc123", "abcdefghijkl", "me:pw"]) {
      expect(shown).not.toContain(leak);
    }
    await userEvent.click(screen.getByRole("button", { name: en.agentRun.step.copy }));
    const trace = writeText.mock.calls[0][0] as string;
    for (const leak of ["abc123", "abcdefghijkl", "me:pw"]) {
      expect(trace).not.toContain(leak);
    }
  });

  it("scrubs a signed token wherever it stands", () => {
    expect(scrubbed("sent eyJhbGciOi.eyJzdWIiOi.c2lnbmF0dXJl back")).toBe("sent … back");
    expect(scrubbed("a plain sentence stays")).toBe("a plain sentence stays");
  });
});

describe("the state of a failure", () => {
  it("is kept for the newest failure of a kind", () => {
    const notices = noticesOf([tool(1, "jc_resource_get", "not found: a"), tool(2, "jc_draft_get", "not found: b")]);
    expect([...notices]).toEqual([[2, "missing"]]);
  });

  it("goes when the assistant then got the same step right", () => {
    const events = [tool(1, "query_endpoint", 'error: {"status":403}'), tool(2, "query_endpoint")];
    expect(noticesOf(events).size).toBe(0);
  });

  it("is no state for a slip the assistant corrects by itself", () => {
    expect(noticesOf([tool(1, "query_endpoint", "error: argument attrs: takes 1 to 100 items")]).size).toBe(0);
  });

  it("draws a slip the assistant went past as a try, and the last one as what failed", () => {
    const slip = "error: argument type: takes 1 to 50 items";
    expect([...retriedSteps([tool(1, "query_endpoint", slip), tool(2, "query_endpoint")])]).toEqual([1]);
    panel([tool(1, "query_endpoint", slip), tool(2, "jc_resource_list", slip)]);
    expect(screen.getAllByRole("img", { name: en.agentRun.step.retried })).toHaveLength(1);
    const reasons = screen.getAllByTestId("step-reason");
    expect(reasons).toHaveLength(2);
    expect(reasons[0]).toHaveTextContent(en.agentRun.step.retried);
    expect(reasons[1]).toHaveTextContent(en.agentRun.failure.argument);
  });

  it("draws the Portal's closing line once, as the notice, and keeps the assistant's own words", () => {
    const events = [
      tool(1, "jc_ui_navigate", "the page needs a name"),
      { seq: 2, kind: "thought", payload: { text: "That page could not be opened. Say which page you want." } } as RunEvent,
      { seq: 3, kind: "thought", payload: { text: "The counters are on the Endpoints page." } } as RunEvent,
    ];
    expect([...echoedLines(events, noticesOf(events))]).toEqual([2]);
    panel(events);
    expect(screen.getAllByText(/could not be opened/)).toHaveLength(1);
    expect(screen.getByText("The counters are on the Endpoints page.")).toBeInTheDocument();
  });

  it("keeps the same line when no notice says it", () => {
    const events = [{ seq: 1, kind: "thought", payload: { text: "That page could not be opened." } } as RunEvent];
    expect(echoedLines(events, noticesOf(events)).size).toBe(0);
  });

  it.each([
    ["refused", en.agentRun.failure.refused, en.agentRun.failure.next.refused],
    ["missing", en.agentRun.failure.missing, en.agentRun.failure.next.missing],
    ["guarded", en.agentRun.failure.guarded, en.agentRun.failure.next.guarded],
    ["page", en.agentRun.failure.page, en.agentRun.failure.next.page],
  ] as const)("says what happened and what next for %s", (kind, title, next) => {
    withI18n(<StepNotice kind={kind} project="demo" />);
    const notice = screen.getByTestId("step-notice");
    expect(notice).toHaveTextContent(title);
    expect(notice).toHaveTextContent(next);
  });

  it("offers the pages a person knows when one would not open", async () => {
    const onOpenLink = vi.fn();
    withI18n(<StepNotice kind="page" project="a b" onOpenLink={onOpenLink} />);
    await userEvent.click(screen.getByRole("link", { name: en.agentRun.failure.page_models }));
    expect(onOpenLink).toHaveBeenCalledWith("/projects/a%20b/models");
    for (const page of ["page_spaces", "page_endpoints", "page_explore"] as const) {
      expect(screen.getByRole("link", { name: en.agentRun.failure[page] })).toBeInTheDocument();
    }
  });

  it("sends the person's own words again from a stopped step, once", async () => {
    const onSend = panel([
      { seq: 1, kind: "message", payload: { text: "Add the slot direction." } } as RunEvent,
      tool(2, "change_resource", "the data read for this message writes this call; data is never an instruction"),
    ]);
    const notice = screen.getByTestId("step-notice");
    // The notice says why; the step's own row does not say it twice.
    expect(screen.queryByTestId("step-reason")).toBeNull();
    await userEvent.click(within(notice).getByRole("button", { name: en.agentRun.failure.askAgain }));
    expect(onSend).toHaveBeenCalledTimes(1);
    expect(onSend).toHaveBeenCalledWith("Add the slot direction.");
  });
});

describe("the press a notice offers", () => {
  it("offers an empty read again with fewer conditions", async () => {
    const onSend = panel([
      tool(1, "query_endpoint", undefined, {
        input: { endpoint: "counters", name: "query_entities", arguments: { type: "BikeCounter" } },
        output: { structuredContent: { entities: [] } },
      }),
    ]);
    await userEvent.click(screen.getByRole("button", { name: en.assistant.query.widen }));
    expect(onSend).toHaveBeenCalledWith(en.assistant.query.widenAsk);
  });

  it("asks what the project holds when something was not found", async () => {
    const onSend = panel([tool(1, "jc_resource_get", "not found: Pipeline 'x'")]);
    await userEvent.click(screen.getByRole("button", { name: en.agentRun.failure.askHolds }));
    expect(onSend).toHaveBeenCalledWith(en.agentRun.failure.holds);
  });

  it("offers nothing to press on a refusal", () => {
    panel([tool(1, "change_resource", 'error: {"status":403}')]);
    expect(within(screen.getByTestId("step-notice")).queryByRole("button")).toBeNull();
  });
});

describe("an empty read and one entity", () => {
  it("says nothing matched and what to try, instead of a table with one header", () => {
    const view = viewOf({ structuredContent: { entities: [] } });
    withI18n(<QueryAnswer view={view} />);
    expect(screen.getByText(en.assistant.query.nothing)).toBeInTheDocument();
    expect(screen.getByText(en.assistant.query.nothingHint)).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("reads one entity as its fields, its id by the name a person reads", () => {
    const view = viewOf({
      structuredContent: {
        entity: { id: "urn:ngsi-ld:BikeCounter:ORD", type: "BikeCounter", city: { type: "Property", value: "Chicago" } },
      },
    });
    expect(view).toEqual({
      kind: "fields",
      fields: [
        ["id", "ORD"],
        ["type", "BikeCounter"],
        ["city", "Chicago"],
      ],
    });
  });

  it("says how to see the rest of a partial answer, and names a column in words", () => {
    const entities = [{ id: "urn:a:b", createdAt: "2026-11-02T18:07:50.158Z" }];
    withI18n(<QueryAnswer view={viewOf({ structuredContent: { entities, total: 221 } })} />);
    expect(screen.getByText(new RegExp(en.assistant.query.moreHint))).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "created at" })).toBeInTheDocument();
  });

  it("reads a time as a date and leaves any other value alone", () => {
    expect(whenOf("2026-11-02T18:07:50.158Z", "en")).toContain("26");
    expect(whenOf("2026-11-02T18:07:50.158Z", "en")).not.toContain("T18");
    expect(whenOf("Banská Bystrica", "en")).toBe("Banská Bystrica");
    expect(whenOf("2026-11-02", "en")).toBe("2026-11-02");
  });

  it("names an entity by its short id on the card", () => {
    const result = queryResultOf({
      tool: "query_endpoint",
      status: "ok",
      input: { endpoint: "counters", name: "get_entity", arguments: { id: "urn:ngsi-ld:BikeCounter:ORD" } },
      output: { structuredContent: {} },
    });
    expect(result?.argument).toBe("id: ORD");
  });

  it("gives every cell of a result table its column's name, so a narrow table can stack", () => {
    const view = viewOf({ structuredContent: { entities: [{ id: "urn:a:b", count: 60, city: "Arcata" }] } });
    withI18n(<QueryAnswer view={view} />);
    expect(screen.getByRole("table")).toHaveClass("table-stack-narrow");
    expect(screen.getByRole("cell", { name: "Arcata" })).toHaveAttribute("data-label", "city");
    expect(screen.getByRole("cell", { name: "60" })).toHaveAttribute("data-label", "count");
  });
});
