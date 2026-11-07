/**
 * The conversation panel (T-0539, AP-53, UI-37).
 *
 * The workspace is the least trusted writer the Portal has, so the panel is tested against what
 * a compromised agent would send: markup in a thought, a question with no schema, a payload
 * missing the field the line is built from.
 */
import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ConversationPanel, answeredSearches, line } from "../src/pages/apps/ConversationPanel";
import { EVENT_KINDS, openQuestions, withdrawnQuestions } from "../src/pages/apps/useAgentRun";
import type { RunEvent } from "../src/pages/apps/useAgentRun";
import { expectNoAxeViolations } from "./page_contract";

const sent = vi.fn();

function panel(events: RunEvent[], streaming = true, building = false) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ConversationPanel
        project="helsinki"
        events={events}
        streaming={streaming}
        answering={false}
        sending={false}
        live
        building={building}
        onAnswer={() => {}}
        onSend={sent}
      />
    </I18nextProvider>,
  );
}

describe("the conversation panel", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    sent.mockClear();
  });

  /// T-1052, UI-39: a turn that only appears on screen is a turn a screen reader misses.
  it("announces new turns as a log, without repeating the history", () => {
    const { container } = panel([{ seq: 1, kind: "message", payload: { text: "42 stations" } }]);

    const log = container.querySelector('[role="log"]');
    expect(log).not.toBeNull();
    expect(log).toHaveAttribute("aria-live", "polite");
    expect(log).toHaveAttribute("aria-atomic", "false");
    expect(log?.textContent).toContain("42 stations");
  });

  /// T-2769: the owner copied "- oimintasuunnitelman" and "helsinki-leoqi" out of the dock. A
  /// long answer and a ten-row grid reach the page whole, every character of them.
  it("draws a long answer and a ten-row grid whole", () => {
    const words = ["Liiketoimintasuunnitelman", "helsinki-af5kileoqi", "Plasma Online Art", "description"];
    const text = Array.from({ length: 140 }, (_, at) => `${words[at % words.length]} ${at}.`).join(" ");
    expect(text.length).toBeGreaterThan(3000);
    const entities = Array.from({ length: 10 }, (_, at) => ({
      id: `urn:ngsi-ld:Event:hel.fi:helsinki:helsinki-af5kileoq${at}`,
      type: "Event",
      name: { type: "LanguageProperty", languageMap: { fi: `Liiketoimintasuunnitelman ilta ${at}`, en: `Business plan evening ${at}` } },
    }));
    const { container } = panel([
      { seq: 1, kind: "message", payload: { text: "what are the events in Helsinki??" } },
      {
        seq: 2,
        kind: "tool",
        payload: {
          tool: "query_endpoint",
          status: "ok",
          input: { endpoint: "helsinki-events", name: "query_entities", arguments: { type: "Event" } },
          output: { structuredContent: { entities } },
        },
      },
      { seq: 3, kind: "thought", payload: { text } },
    ]);
    const shown = container.textContent ?? "";
    expect(shown).toContain(text);
    for (let at = 0; at < 10; at++) {
      expect(shown).toContain(`Business plan evening ${at}`);
      expect(shown).toContain(`helsinki-af5kileoq${at}`);
    }
  });

  it("draws the grid's language values in the reader's language", async () => {
    // The Portal's own languages; a reader in one of them reads the value in it (T-2769).
    await i18n.changeLanguage("de");
    const { container } = panel([
      {
        seq: 1,
        kind: "tool",
        payload: {
          tool: "query_endpoint",
          status: "ok",
          input: { endpoint: "helsinki-events", name: "query_entities", arguments: { type: "Event" } },
          output: {
            structuredContent: {
              entities: [
                {
                  id: "urn:ngsi-ld:Event:hel.fi:helsinki:e1",
                  type: "Event",
                  name: { type: "LanguageProperty", languageMap: { en: "Business plan evening", de: "Abend des Geschäftsplans" } },
                },
              ],
            },
          },
        },
      },
    ]);
    // The card, not the step's raw output behind Details, which keeps the answer as it came.
    const card = container.querySelector('[data-testid="query-result"]');
    expect(card?.textContent).toContain("Abend des Geschäftsplans");
    expect(card?.textContent).not.toContain("Business plan evening");
  });

  it("renders markup from the workspace as text, never as markup (AP-53)", () => {
    const { container } = panel([
      {
        seq: 1,
        kind: "thought",
        payload: { text: "<img src=x onerror=alert(1)><script>alert(2)</script>" },
      },
    ]);

    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("img")).toBeNull();
    expect(
      screen.getByText("<img src=x onerror=alert(1)><script>alert(2)</script>"),
    ).toBeInTheDocument();
  });

  it("does not break on a payload missing the field the line needs", () => {
    panel([
      { seq: 1, kind: "tool", payload: {} },
      { seq: 2, kind: "commit", payload: { sha: 42 } },
      { seq: 3, kind: "somethingNew", payload: { text: "from a newer Portal" } },
    ]);

    const rows = within(
      screen.getByRole("list", { name: en.agentRun.conversation.title }),
    ).getAllByRole("listitem");
    // A kind this Portal has no words for draws nothing: a kind name is machinery (T-0772).
    expect(rows).toHaveLength(2);
    expect(screen.queryByText(/somethingNew/)).not.toBeInTheDocument();
  });

  it("leaves the page the assistant opened to the dock's notice, never a line with the event's kind (T-0772)", () => {
    panel([
      { seq: 1, kind: "thought", payload: { text: "Drafted the grant." } },
      {
        seq: 2,
        kind: "navigate",
        payload: { route: "/projects/helsinki/access?grant=jana-kovacova-steward-helsinki" },
      },
    ]);

    const rows = within(
      screen.getByRole("list", { name: en.agentRun.conversation.title }),
    ).getAllByRole("listitem");
    expect(rows).toHaveLength(1);
    expect(screen.queryByText(/navigate/i)).not.toBeInTheDocument();
  });

  it("never puts the name of an event's kind on screen", () => {
    const t = i18n.t.bind(i18n) as (key: string, options?: Record<string, unknown>) => string;
    for (const kind of EVENT_KINDS) {
      expect(line({ seq: 1, kind, payload: {} }, t), kind).not.toBe(kind);
    }
  });

  it("keeps the turns in stream order, each under whoever said it", () => {
    panel([
      { seq: 1, kind: "thought", payload: { text: "first" } },
      { seq: 2, kind: "message", payload: { text: "second", sentBy: "jana.kovacova" } },
      { seq: 3, kind: "thought", payload: { text: "third" } },
    ]);

    const list = screen.getByRole("list", { name: en.agentRun.conversation.title });
    const rows = within(list).getAllByRole("listitem");
    expect(rows.map((row) => row.textContent)).toEqual([
      `${en.agentRun.conversation.agent}first`,
      "jana.kovacovasecond",
      `${en.agentRun.conversation.agent}third`,
    ]);
  });

  it("draws the shell work as activity rather than as something the agent said", () => {
    panel([
      { seq: 1, kind: "thought", payload: { text: "Reading the endpoint." } },
      { seq: 2, kind: "tool", payload: { tool: "bash", command: "curl …", exitCode: 0 } },
    ]);

    const rows = within(
      screen.getByRole("list", { name: en.agentRun.conversation.title }),
    ).getAllByRole("listitem");
    expect(rows[0]).toHaveTextContent(en.agentRun.conversation.agent);
    // A tool line is an inspectable step named after its tool, with no speaker at all (AG-56).
    expect(rows[1]).toHaveTextContent("bash");
    expect(rows[1]).not.toHaveTextContent(en.agentRun.conversation.agent);
  });

  it("draws the run's status changes as one progress line, never as lines of their own", () => {
    const { unmount } = panel([
      { seq: 1, kind: "status", payload: { status: "queued" } },
      { seq: 2, kind: "status", payload: { status: "starting" } },
      { seq: 3, kind: "status", payload: { status: "interviewing" } },
      { seq: 4, kind: "thought", payload: { text: "Which feed?" } },
    ]);
    expect(screen.queryByText(/^Now /)).not.toBeInTheDocument();
    expect(screen.getAllByTestId("run-progress")).toHaveLength(1);
    expect(screen.getByTestId("run-progress")).toHaveTextContent(en.agentRun.progress.waiting);
    unmount();

    panel([
      { seq: 3, kind: "status", payload: { status: "interviewing" } },
      { seq: 4, kind: "message", payload: { text: "The HSL one" } },
    ]);
    expect(screen.getByTestId("run-progress")).toHaveTextContent(en.agentRun.progress.working);
  });

  it("keeps commits and preview addresses behind Details and token counts out of the conversation", async () => {
    const user = userEvent.setup();
    panel([
      { seq: 1, kind: "thought", payload: { text: "Building the map." } },
      { seq: 2, kind: "usage", payload: { tokensThisStep: 57285 } },
      { seq: 3, kind: "commit", payload: { sha: "3e26835f00", message: "map" } },
      { seq: 4, kind: "preview", payload: { previewUrl: "/api/v1/projects/helsinki/apps/x/preview" } },
    ]);
    expect(screen.queryByText(/57285/)).toBeNull();
    const commit = screen.getByText(/3e26835f/);
    expect(commit).not.toBeVisible();
    await user.click(screen.getByText("Details (2 lines)"));
    expect(commit).toBeVisible();
    expect(screen.queryByText(/57285/)).toBeNull();
    // The preview is a link a person clicks, never the API path printed (T-0703).
    expect(screen.getByRole("link", { name: new RegExp(`^${en.agentRun.line.previewLink}`) })).toBeVisible();
  });

  it("shows a single machinery line as itself, never as an empty Details (T-2769)", () => {
    panel([
      { seq: 1, kind: "thought", payload: { text: "Drafted the endpoint." } },
      { seq: 2, kind: "commit", payload: { sha: "3e26835f00", message: "map" } },
    ]);
    expect(screen.getByText(/3e26835f/)).toBeVisible();
    expect(screen.queryByText(/^Details/)).toBeNull();
  });

  it("names what a read read: how many of how many, of which type, from where (T-2769)", () => {
    const entities = (count: number) =>
      Array.from({ length: count }, (_, at) => ({ id: `urn:ngsi-ld:Event:hel.fi:helsinki:e${at}`, type: "Event" }));
    const read = (seq: number, output: unknown, type?: string) => ({
      seq,
      kind: "tool",
      payload: {
        tool: "query_endpoint",
        status: "ok",
        input: { endpoint: "helsinki-events", name: "query_entities", arguments: type ? { type } : {} },
        output,
      },
    });
    panel([
      read(1, { structuredContent: { entities: entities(10), total: 110 } }, "Event"),
      { seq: 2, kind: "thought", payload: { text: "Here they are." } },
      read(3, { structuredContent: { entities: entities(3) } }),
      { seq: 4, kind: "thought", payload: { text: "And these." } },
      read(5, { structuredContent: { count: 110 } }),
    ]);
    expect(screen.getByText("Read 10 of 110 Event from helsinki-events")).toBeInTheDocument();
    expect(screen.getByText("Read 3 entities from helsinki-events")).toBeInTheDocument();
    expect(screen.getByText("Read helsinki-events")).toBeInTheDocument();
  });

  it("names the assistant's own steps by what they did", () => {
    panel([
      { seq: 1, kind: "tool", payload: { tool: "query_endpoint", status: "ok", input: { endpoint: "helsinki-all" } } },
      { seq: 2, kind: "tool", payload: { tool: "change_resource", status: "ok", input: { kind: "Pipeline" } } },
    ]);
    expect(screen.getByText("Read helsinki-all")).toBeInTheDocument();
    expect(screen.getByText(en.agentRun.step.label.change_resource)).toBeInTheDocument();
  });

  it("shows what the catalog search found only when the answer was about it", () => {
    const search = (seq: number): RunEvent => ({ seq, kind: "tool", payload: { tool: "search_catalog", status: "ok", output: { items: [] } } });
    const integrate: RunEvent[] = [
      { seq: 1, kind: "message", payload: { text: "Integrate the HSL feed" } },
      search(2),
      { seq: 3, kind: "tool", payload: { tool: "space_complete", status: "ok" } },
      { seq: 4, kind: "thought", payload: { text: "Drafted the space." } },
    ];
    expect([...answeredSearches(integrate)]).toEqual([]);
    panel(integrate);
    expect(screen.queryByText(en.agentRun.catalog.none)).not.toBeInTheDocument();

    expect([...answeredSearches([search(2), { seq: 3, kind: "thought", payload: { text: "Yes." } }])]).toEqual([2]);
    // A search nothing has followed yet is still what the conversation is about.
    expect([...answeredSearches([search(5)])]).toEqual([5]);
  });

  it("says whether the stream is live, so a stalled run is not read as a quiet one", () => {
    const { unmount } = panel([], true);
    expect(screen.getByText(en.agentRun.conversation.live)).toBeInTheDocument();
    unmount();

    panel([], false);
    expect(screen.getByText(en.agentRun.conversation.offline)).toBeInTheDocument();
  });

  it("shows nothing-yet rather than an empty box before the first frame", () => {
    panel([]);
    expect(screen.getByText(en.agentRun.conversation.empty)).toBeInTheDocument();
  });

  it("sends an instruction to a live run and empties the box", async () => {
    const user = userEvent.setup();
    panel([]);

    await user.type(
      screen.getByLabelText(en.agentRun.conversation.placeholder),
      "Sort by free bikes",
    );
    await user.click(screen.getByRole("button", { name: en.agentRun.conversation.send }));

    expect(sent).toHaveBeenCalledWith("Sort by free bikes");
    expect(screen.getByLabelText(en.agentRun.conversation.placeholder)).toHaveValue("");
  });

  /// T-2772: a turn that ended without an answer offers the same message again, in this
  /// conversation, one press; an answered turn and an older failure offer nothing.
  it("offers Try again on a failed answer and sends the last message again", async () => {
    const asked = { seq: 1, kind: "message", payload: { text: "how many bikes are free?", sentBy: "jana" } };
    const failed = {
      seq: 2,
      kind: "thought",
      payload: { text: "The answer failed: the model service did not answer in time.", failed: true },
    };
    panel([asked, failed]);
    expect(screen.getByText(failed.payload.text)).toBeInTheDocument();
    expect(screen.getByText(en.agentRun.conversation.unanswered)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: en.agentRun.conversation.tryAgain }));
    expect(sent).toHaveBeenCalledWith("how many bikes are free?");
  });

  it("offers no Try again once the failed answer is followed by an answer", () => {
    panel([
      { seq: 1, kind: "message", payload: { text: "bikes?", sentBy: "jana" } },
      { seq: 2, kind: "thought", payload: { text: "The answer failed: busy.", failed: true } },
      { seq: 3, kind: "message", payload: { text: "bikes?", sentBy: "jana" } },
      { seq: 4, kind: "thought", payload: { text: "12 stations have free bikes." } },
    ]);
    expect(screen.queryByRole("button", { name: en.agentRun.conversation.tryAgain })).toBeNull();
    expect(screen.queryByText(en.agentRun.conversation.unanswered)).toBeNull();
  });

  /// T-2772: an ended conversation says who or what ended it, as the status said.
  it("says why an ended conversation ended", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <ConversationPanel
          project="helsinki"
          events={[
            { seq: 1, kind: "message", payload: { text: "bikes?", sentBy: "jana" } },
            { seq: 2, kind: "status", payload: { status: "cancelled", reason: "cancelled by jana.kovacova" } },
          ]}
          streaming
          answering={false}
          sending={false}
          live={false}
          onAnswer={() => {}}
          onSend={sent}
        />
      </I18nextProvider>,
    );
    expect(screen.getByTestId("ended-because")).toHaveTextContent(
      en.agentRun.conversation.endedBecause.replace("{reason}", "cancelled by jana.kovacova"),
    );
  });

  it("will not send whitespace", async () => {
    const user = userEvent.setup();
    panel([]);

    await user.type(screen.getByLabelText(en.agentRun.conversation.placeholder), "   ");

    expect(screen.getByRole("button", { name: en.agentRun.conversation.send })).toHaveAttribute("aria-disabled", "true");
    expect(sent).not.toHaveBeenCalled();
  });

  it("shows what the person already said, as text and not as markup", () => {
    const { container } = panel([
      {
        seq: 1,
        kind: "message",
        payload: { text: "<b>use a map</b>", sentBy: "jana.kovacova" },
      },
    ]);

    expect(container.querySelector("b")).toBeNull();
    expect(screen.getByText("<b>use a map</b>")).toBeInTheDocument();
    expect(screen.getByText("jana.kovacova")).toBeInTheDocument();
  });

  it("sends on Enter, and keeps Shift+Enter for a second line", async () => {
    const user = userEvent.setup();
    panel([]);
    const box = screen.getByLabelText(en.agentRun.conversation.placeholder);

    await user.type(box, "a bar chart{Shift>}{Enter}{/Shift}per station");
    expect(sent).not.toHaveBeenCalled();
    expect(box).toHaveValue("a bar chart\nper station");

    await user.type(box, "{Enter}");
    expect(sent).toHaveBeenCalledWith("a bar chart\nper station");
  });

  it("offers no box once the run is over", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <ConversationPanel
        project="helsinki"
          events={[]}
          streaming={false}
          answering={false}
          sending={false}
          live={false}
          onAnswer={() => {}}
          onSend={sent}
        />
      </I18nextProvider>,
    );

    expect(
      screen.queryByLabelText(en.agentRun.conversation.placeholder),
    ).not.toBeInTheDocument();
    expect(screen.getByText(en.agentRun.conversation.closed)).toBeInTheDocument();
  });

  it("renders the agent's question through the Portal's own form stack", () => {
    panel([
      {
        seq: 1,
        kind: "question",
        payload: {
          questionId: "q-refresh",
          schema: {
            type: "object",
            properties: { seconds: { type: "integer", title: "Refresh every" } },
          },
        },
      },
    ]);

    expect(screen.getByText(en.agentRun.conversation.question)).toBeInTheDocument();
    expect(screen.getByLabelText(/Refresh every/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.agentRun.conversation.answer })).toBeEnabled();
  });
});

describe("openQuestions", () => {
  const question = (seq: number, id: string): RunEvent => ({
    seq,
    kind: "question",
    payload: { questionId: id, schema: { type: "object" } },
  });

  it("keeps the unanswered ones, oldest first", () => {
    const events = [question(1, "a"), question(2, "b"), { seq: 3, kind: "answer", payload: { questionId: "a" } }];
    expect(openQuestions(events).map((q) => q.questionId)).toEqual(["b"]);
  });

  /// T-6104: "cancel this" typed under "Where does the data come from?" left the form open.
  it("closes a question the person wrote past instead of answering", () => {
    const typed = (seq: number, sentBy: string): RunEvent => ({ seq, kind: "message", payload: { text: "cancel this", sentBy } });
    expect(openQuestions([question(1, "a"), typed(2, "demo.steward")])).toEqual([]);
    // The agent's own words close nothing, and a question asked after the message stays open.
    expect(openQuestions([question(1, "a"), typed(2, "agent")]).map((q) => q.questionId)).toEqual(["a"]);
    expect(openQuestions([typed(1, "demo.steward"), question(2, "b")]).map((q) => q.questionId)).toEqual(["b"]);
  });

  it("never calls an answered question withdrawn when the person writes on", () => {
    const events: RunEvent[] = [
      question(1, "a"),
      { seq: 2, kind: "answer", payload: { questionId: "a" } },
      { seq: 3, kind: "message", payload: { text: "thanks, go on", sentBy: "demo.steward" } },
    ];
    expect([...withdrawnQuestions(events)]).toEqual([]);
    expect(openQuestions(events)).toEqual([]);
  });

  it("drops the form and says the question was withdrawn after a typed message", () => {
    panel([
      { seq: 1, kind: "question", payload: { questionId: "q1", schema: { type: "object", title: "Where does the data come from?" } } },
      { seq: 2, kind: "message", payload: { text: "cancel this", sentBy: "demo.steward" } },
      { seq: 3, kind: "thought", payload: { text: "Pipeline setup cancelled." } },
    ]);
    expect(screen.queryByText(en.agentRun.conversation.question)).toBeNull();
    expect(screen.getByText(en.agentRun.line.withdrawn)).toBeInTheDocument();
  });

  it("marks a question the platform filled, whose answer it checks", () => {
    const picked = { ...question(1, "a"), payload: { questionId: "a", schema: { type: "object" }, pick: "endpoints" } };
    expect(openQuestions([picked])[0].pick).toBe(true);
    expect(openQuestions([question(2, "b")])[0].pick).toBe(false);
  });

  it("keeps an answered question in the conversation by the titles chosen, and no longer answerable", () => {
    const schema = {
      type: "object",
      title: "Which endpoints?",
      properties: {
        answer: {
          type: "array",
          items: { oneOf: [{ const: "bikes", title: "City bikes" }, { const: "air", title: "Air quality" }] },
        },
      },
    };
    panel([
      { seq: 1, kind: "question", payload: { questionId: "q1", schema, pick: "endpoints", multiple: true } },
      { seq: 2, kind: "answer", payload: { questionId: "q1", answers: { answer: ["bikes", "air"] } } },
    ]);

    expect(screen.getByText("You chose: City bikes, Air quality")).toBeInTheDocument();
    expect(screen.queryByTestId("question-options")).toBeNull();
  });

  it("ignores a question with no schema to render", () => {
    expect(
      openQuestions([{ seq: 1, kind: "question", payload: { questionId: "a" } }]),
    ).toEqual([]);
  });
});

describe("a build run reads as a build (T-0703)", () => {
  const SEARCH: RunEvent[] = [
    { seq: 1, kind: "message", payload: { text: "Build me a map of the bike stations." } },
    {
      seq: 2,
      kind: "tool",
      payload: {
        tool: "search_catalog",
        input: { q: "bikes" },
        output: {
          items: [
            { kind: "Endpoint", name: "helsinki-bikes", project: "helsinki", title: "City bikes" },
          ],
        },
      },
    },
    { seq: 3, kind: "thought", payload: { text: "Building with the bikes endpoint." } },
  ];

  it("folds the catalog cards of a search the person never asked for", () => {
    panel(SEARCH, true, true);
    // The step's own trace, folded in its <details>, still holds the raw output (T-5516).
    const cards = screen.queryAllByText("City bikes").filter((node) => node.closest("details:not([open])") === null);
    expect(cards).toEqual([]);
  });

  it("still shows them when the search is the answer to a question", () => {
    panel(SEARCH, true, false);
    expect(screen.getByText(/City bikes/)).toBeInTheDocument();
  });

  it("offers the preview as a link, not as an API path", async () => {
    panel([
      { seq: 1, kind: "thought", payload: { text: "Here it is." } },
      {
        seq: 2,
        kind: "preview",
        payload: { previewUrl: "/api/v1/projects/helsinki/agent-runs/r1/preview?v=1" },
      },
    ]);
    await userEvent.click(screen.getByText(/detail/i));
    const link = screen.getByRole("link", { name: new RegExp(`^${en.agentRun.line.previewLink}`) });
    expect(link).toHaveAttribute("href", "/api/v1/projects/helsinki/agent-runs/r1/preview?v=1");
    expect(screen.queryByText(/preview\?v=1$/)).toBeNull();
  });

  it("a preview address the workspace chose the scheme of is never a link", async () => {
    // PF-50. `previewUrl` is written by the workspace, the least trusted writer this panel has,
    // and went into `href` unchecked: a `javascript:` preview ran on the Portal's own origin
    // with the reader's session the moment they clicked "open preview". The words stay; only
    // the link is withheld.
    panel([
      { seq: 1, kind: "preview", payload: { previewUrl: "javascript:fetch('/api/v1/projects')" } },
    ]);
    await userEvent.click(screen.getByText(/detail/i));
    expect(screen.queryByRole("link", { name: new RegExp(`^${en.agentRun.line.previewLink}`) })).toBeNull();
    expect(screen.getByText(en.agentRun.line.previewLink)).toBeVisible();
  });

  it("a preview address the server really served is still a link", async () => {
    panel([
      { seq: 1, kind: "preview", payload: { previewUrl: "/api/v1/projects/helsinki/apps/x/preview" } },
    ]);
    await userEvent.click(screen.getByText(/detail/i));
    expect(screen.getByRole("link", { name: new RegExp(`^${en.agentRun.line.previewLink}`) })).toHaveAttribute(
      "href",
      "/api/v1/projects/helsinki/apps/x/preview",
    );
  });
});

// A person reads what was asked and answered, never a question's id (UI-16), and a question the
// assistant says in its next line is read once.
describe("questions and answers in the transcript", () => {
  const t = i18n.t.bind(i18n);
  const question = (seq: number, title?: string): RunEvent => ({
    seq,
    kind: "question",
    payload: {
      questionId: "q-2026-09-25T020428.100602538Z",
      schema: title === undefined ? {} : { type: "object", title, properties: { answer: { type: "string", title } } },
    },
  });

  it("says a question by its words, or that one was asked", () => {
    expect(line(question(1, "Which space should it land in?"), t)).toBe("Asked: Which space should it land in?");
    expect(line(question(1), t)).toBe(en.agentRun.line.asked);
  });

  it("says a form's answer by what was typed, and never the question's id", () => {
    const answer = (answers: unknown): RunEvent => ({
      seq: 2,
      kind: "answer",
      payload: { questionId: "q-2026-09-25T020428.100602538Z", answers },
    });
    expect(line(answer({ center: "Kallio" }), t)).toBe("You answered: Kallio");
    expect(line(answer({}), t)).toBe(en.agentRun.line.answered);
    expect(line(answer({ answer: "space" }), t, new Map([["space", "A context space"]]))).toBe(
      "You chose: A context space",
    );
  });

  it("reads a question the assistant says next only once, and one it does not say, once", () => {
    panel([
      question(1, "Where does the data come from?"),
      { seq: 2, kind: "thought", payload: { text: "Where does the data come from?" } },
      { seq: 3, kind: "answer", payload: { questionId: "q-2026-09-25T020428.100602538Z", answers: { answer: "space" } } },
      question(4, "Which space should it land in?"),
    ]);
    expect(screen.getAllByText(/Where does the data come from\?/)).toHaveLength(1);
    expect(screen.getByText("Asked: Which space should it land in?")).toBeInTheDocument();
    expect(screen.queryByText(/q-2026-09-25T/)).toBeNull();
  });
});

/// T-2821, API/04 §4: a `partial` is the assistant's words while the model still writes them. It
/// shows only while it is the newest event, the answer replaces it, and the log announces the
/// answer once rather than every half of it.
describe("words the model is still writing", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  const asked: RunEvent = { seq: 1, kind: "message", payload: { text: "which stations are empty?", sentBy: "jana" } };

  it("draws the newest partial as the assistant's line, hidden from the announcements", () => {
    panel([
      { seq: 0, kind: "status", payload: { status: "interviewing" } },
      asked,
      { seq: 2, kind: "partial", payload: { text: "Two stations", elapsedMs: 900 } },
      { seq: 3, kind: "partial", payload: { text: "Two stations are empty", elapsedMs: 1400 } },
    ]);
    const lines = screen.getAllByTestId("partial-line");
    expect(lines).toHaveLength(1);
    expect(lines[0]).toHaveTextContent("Two stations are empty");
    expect(lines[0]).toHaveTextContent(en.agentRun.conversation.agent);
    expect(lines[0]).toHaveAttribute("aria-hidden", "true");
    expect(screen.getByTestId("run-progress")).toHaveTextContent(en.agentRun.progress.working);
  });

  it("gives way to the answer, and to a step, that follows it", () => {
    const { rerender } = panel([
      asked,
      { seq: 2, kind: "partial", payload: { text: "Let me look.", elapsedMs: 700 } },
      { seq: 3, kind: "tool", payload: { tool: "search_catalog", status: "ok", input: { q: "stations" } } },
    ]);
    expect(screen.queryByTestId("partial-line")).not.toBeInTheDocument();
    expect(screen.queryByText("Let me look.")).not.toBeInTheDocument();

    rerender(
      <I18nextProvider i18n={i18n}>
        <ConversationPanel
          project="helsinki"
          events={[
            asked,
            { seq: 2, kind: "partial", payload: { text: "Two stations are", elapsedMs: 700 } },
            { seq: 3, kind: "thought", payload: { text: "Two stations are empty." } },
          ]}
          streaming
          answering={false}
          sending={false}
          live
          onAnswer={() => {}}
          onSend={sent}
        />
      </I18nextProvider>,
    );
    expect(screen.queryByTestId("partial-line")).not.toBeInTheDocument();
    expect(screen.getByText("Two stations are empty.")).toBeInTheDocument();
    expect(screen.queryByText("Two stations are")).not.toBeInTheDocument();
  });

  it("never counts the words before a step as the answer about a search", () => {
    const events: RunEvent[] = [
      asked,
      { seq: 2, kind: "tool", payload: { tool: "search_catalog", status: "ok" } },
      { seq: 3, kind: "partial", payload: { text: "Reading it." } },
      { seq: 4, kind: "tool", payload: { tool: "query_endpoint", status: "ok" } },
    ];
    expect(answeredSearches(events).has(2)).toBe(false);
  });

  it("renders a partial's markup as text", () => {
    panel([asked, { seq: 2, kind: "partial", payload: { text: "<img src=x onerror=alert(1)>" } }]);
    expect(screen.getByTestId("partial-line")).toHaveTextContent("<img src=x onerror=alert(1)>");
    expect(document.querySelector("img[src='x']")).toBeNull();
  });

  // T-3986: a model run draws these turns, which the live walk cannot start (spend guard).
  it("has_no_serious_axe_violation_as_a_model_run_draws_it", async () => {
    const { container } = panel([
      { seq: 1, kind: "message", payload: { text: "## Stations\n\n- Töölö\n\n| Station | Free bikes |\n|---|---|\n| Töölö | 0 |" } },
    ]);
    await expectNoAxeViolations(container);
  });
});
