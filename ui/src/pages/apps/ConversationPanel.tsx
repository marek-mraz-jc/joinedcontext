import { useEffect, useRef, useState } from "react";
import type { JSX, ReactNode } from "react";
import { clsx } from "clsx";
import { useTranslation } from "react-i18next";
import { forPeople } from "../../api/client";
import { choicesOf, QuestionOptions } from "./QuestionOptions";
import { AppBuildCard, appBuildOf } from "./AppBuildCard";
import type { JsonSchema } from "../../components/forms/types";
import { Alert, Button, EmptyState, ExternalLink, Textarea } from "../../components/ui";
import { openQuestions, TERMINAL_STATES, withdrawnQuestions } from "./useAgentRun";
import { ActionStep, echoedLines, noticesOf, retriedSteps, StepNotice } from "./ActionStep";
import { CatalogCards, catalogDatasetsOf, catalogItemsOf } from "./CatalogCards";
import { ChangeFollowCard, followedChangeOf } from "./ChangeFollowCard";
import { ChangeTestCard, changeTestOf } from "./ChangeTestCard";
import { EndpointProposalCard, proposalOf } from "./EndpointProposalCard";
import { EntityWriteCard, entityWriteOf } from "./EntityWriteCard";
import { KpiCard, kpiOf } from "./KpiCard";
import { KpiPipelineCard, kpiPipelineOf } from "./KpiPipelineCard";
import { QueryResultCard, queryResultOf } from "./QueryResultCard";
import { QuestionData } from "./QuestionData";
import { Prose } from "./Prose";
import type { OpenLink } from "./Prose";
import type { RunEvent } from "./useAgentRun";

/** Who a line came from. The three read differently, so they are drawn differently. */
export type Speaker = "agent" | "person" | "activity";

export function speakerOf(kind: string): Speaker {
  if (kind === "thought" || kind === "question" || kind === "partial") {
    return "agent";
  }
  if (kind === "message" || kind === "answer") {
    return "person";
  }
  return "activity";
}

/** Lines about the machinery of a run rather than the conversation, kept behind Details. */
const DETAIL_KINDS = new Set(["commit", "preview", "lag"]);

/** Events that are not lines of the transcript at all; what a run cost is on its timeline. */
const UNDRAWN_KINDS = new Set(["status", "navigate", "ui", "usage"]);

/** Where a run stands, as the one line a person reads instead of every status change. */
export type Progress = "working" | "waiting" | "done" | "stopped";

export function progressOf(events: RunEvent[]): Progress | null {
  const status = events.filter((event) => event.kind === "status").at(-1)?.payload.status;
  if (typeof status !== "string") {
    return null;
  }
  if (status === "published") {
    return "done";
  }
  if (TERMINAL_STATES.includes(status)) {
    return "stopped";
  }
  // The model is still writing the words on screen (T-2821).
  if (events.at(-1)?.kind === "partial") {
    return "working";
  }
  const lastTurn = events.filter((event) => speakerOf(event.kind) !== "activity").at(-1);
  if (lastTurn !== undefined && speakerOf(lastTurn.kind) === "person") {
    return "working";
  }
  return ["interviewing", "previewing", "awaiting_approval"].includes(status) ? "waiting" : "working";
}

/**
 * The catalog searches whose cards stay in the conversation: those the agent's next words were
 * about with no other step between, and the latest one while nothing has followed it yet. A
 * search on the way to integrating a feed is only a step.
 */
export function answeredSearches(events: RunEvent[]): Set<number> {
  const answered = new Set<number>();
  let search: number | null = null;
  for (const event of events) {
    // Words the model wrote on its way to its next step are not an answer about the search.
    if (event.kind === "partial") {
      continue;
    }
    if (event.kind === "tool") {
      search = event.payload.tool === "search_catalog" ? event.seq : null;
    } else if (speakerOf(event.kind) === "agent" && search !== null) {
      answered.add(search);
      search = null;
    }
  }
  if (search !== null) {
    answered.add(search);
  }
  return answered;
}

/** The words a catalog search ran on, as its step's input carries them. */
function wordsOf(input: unknown): string {
  const q = typeof input === "object" && input !== null ? (input as { q?: unknown }).q : undefined;
  return typeof q === "string" ? q : "";
}

/** A row of the transcript: one event, or the machinery lines between two of them. */
type Row = { event: RunEvent; count: number } | { details: RunEvent[] };

/** The words a question asked, as its schema titles them; "" when it carries none. */
export function questionTitle(payload: Record<string, unknown>): string {
  const schema = payload.schema as { title?: unknown; properties?: { answer?: { title?: unknown } } } | undefined;
  const title = schema?.title ?? schema?.properties?.answer?.title;
  return typeof title === "string" ? title : "";
}

/**
 * The events a transcript draws: not the status changes (the progress line stands for them), not
 * the pages opened (the dock's notice does), not a question the assistant says in its very next
 * line, which would read twice, and a `partial` only while it is the newest event: the answer or
 * the step that follows it stands for it (API/04 §4, T-2821).
 */
export function drawnEvents(events: RunEvent[]): RunEvent[] {
  const newest = events.at(-1);
  const shown = events.filter(
    (event) => !UNDRAWN_KINDS.has(event.kind) && (event.kind !== "partial" || event === newest),
  );
  return shown.filter((event, at) => {
    if (event.kind !== "question") {
      return true;
    }
    const next = shown[at + 1];
    return !(next?.kind === "thought" && next.payload.text === questionTitle(event.payload) && questionTitle(event.payload) !== "");
  });
}

function rowsOf(events: RunEvent[]): Row[] {
  const rows: Row[] = [];
  for (const folded of foldRepeats(drawnEvents(events))) {
    const last = rows.at(-1);
    if (!DETAIL_KINDS.has(folded.event.kind)) {
      rows.push(folded);
    } else if (last !== undefined && "details" in last) {
      last.details.push(folded.event);
    } else {
      rows.push({ details: [folded.event] });
    }
  }
  return rows;
}

/** What makes two tool steps the same step: the tool, how it ended and why. */
function stepKey(event: RunEvent): string {
  const { tool, status, error, exitCode } = event.payload;
  return JSON.stringify([tool, status, error ?? null, exitCode ?? null]);
}

/**
 * The events as the transcript draws them: a tool step repeated in a row with the same outcome
 * is one step with a count, the newest occurrence standing for all of them, so a function the
 * preview calls on every render does not bury the conversation. A repeat after anything else
 * starts a new step.
 */
export function foldRepeats(events: RunEvent[]): { event: RunEvent; count: number }[] {
  const folded: { event: RunEvent; count: number }[] = [];
  for (const event of events) {
    const last = folded.at(-1);
    if (
      last !== undefined &&
      event.kind === "tool" &&
      last.event.kind === "tool" &&
      stepKey(event) === stepKey(last.event)
    ) {
      folded[folded.length - 1] = { event, count: last.count + 1 };
    } else {
      folded.push({ event, count: 1 });
    }
  }
  return folded;
}

/**
 * One line of the run, as text.
 *
 * Every value here comes from the workspace, which is the least trusted writer the Portal has:
 * a payload is rendered as React children and never as markup, so a `<script>` in a thought is
 * a `<script>` on the screen and nothing else (AP-53).
 */
export function line(
  event: RunEvent,
  t: (key: string, options?: Record<string, unknown>) => string,
  titles: Map<string, string> = new Map(),
): string {
  const payload = event.payload;
  const text = (key: string): string => {
    const value = payload[key];
    return typeof value === "string" ? value : "";
  };
  switch (event.kind) {
    case "status":
      return t("agentRun.line.status", { status: text("status") });
    case "thought":
    case "partial":
      return text("text");
    case "message":
      return text("text");
    case "tool":
      return t("agentRun.line.tool", {
        tool: text("tool"),
        command: text("command"),
        exit: String(payload.exitCode ?? ""),
      });
    case "commit":
      return t("agentRun.line.commit", {
        sha: text("sha").slice(0, 8),
        message: text("message"),
      });
    case "preview":
      return t("agentRun.line.preview", { url: text("previewUrl") });
    case "tests": {
      const version = String(payload.version ?? "");
      switch (payload.outcome) {
        case "running":
          return t("agentRun.line.testsRunning", { version });
        case "passed":
          return t("agentRun.line.testsPassed", { version, passed: String(payload.passed ?? 0) });
        case "failed":
          return t("agentRun.line.testsFailed", {
            version,
            failed: String(payload.failed ?? 0),
            names: (Array.isArray(payload.failures) ? payload.failures : [])
              .map((failure: unknown) => (failure as { name?: unknown } | null)?.name)
              .filter((name): name is string => typeof name === "string")
              .join("; "),
          });
        case "error":
          return t("agentRun.line.testsError", { version, reason: text("reason") });
        default:
          return t("agentRun.line.testsSkipped", { version, reason: text("reason") });
      }
    }
    case "answer": {
      const answers = payload.answers as { answer?: unknown; file?: { name?: unknown }; url?: unknown } | undefined;
      // Data handed over is named, never shown: the file's text stays on the run (T-2694).
      if (typeof answers?.file?.name === "string") {
        return t("agentRun.line.file", { name: answers.file.name });
      }
      if (typeof answers?.url === "string") {
        return t("agentRun.line.url", { url: answers.url });
      }
      // What was chosen stays in the conversation, by the titles the person read (UI-74).
      const answer = answers?.answer;
      const chosen = (Array.isArray(answer) ? answer : [answer]).filter(
        (one): one is string => typeof one === "string",
      );
      if (chosen.length > 0) {
        return t("agentRun.line.chose", { choice: chosen.map((one) => titles.get(one) ?? one).join(", ") });
      }
      // A form's answer: what was typed, never the question's id (UI-16).
      const typed = Object.values(answers ?? {}).filter((one): one is string => typeof one === "string" && one !== "");
      return typed.length > 0 ? t("agentRun.line.answer", { answer: typed.join(", ") }) : t("agentRun.line.answered");
    }
    case "question": {
      const title = questionTitle(payload);
      return title !== "" ? t("agentRun.line.question", { question: title }) : t("agentRun.line.asked");
    }
    case "lag":
      return t("agentRun.line.lag", { missed: String(payload.missed ?? "") });
    case "endpoints": {
      const names = Array.isArray(payload.names)
        ? payload.names.filter((name): name is string => typeof name === "string")
        : [];
      return names.length > 0
        ? t("agentRun.line.endpoints", { names: names.join(", ") })
        : t("agentRun.line.noEndpoints");
    }
    default:
      return "";
  }
}

/** Whose name goes above a bubble: the person who typed it, or the agent. */
function labelOf(
  event: RunEvent,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  const who = event.payload.sentBy;
  if (typeof who === "string" && who !== "") {
    return who;
  }
  return speakerOf(event.kind) === "person"
    ? t("agentRun.conversation.you")
    : t("agentRun.conversation.agent");
}

/**
 * The conversation with the agent: what it said, what it did, the questions it is waiting on,
 * and the box a person writes the next instruction in.
 *
 * A run is a conversation rather than a form, which is why the composer is open the whole time
 * and not only while a question stands: somebody watching a build go the wrong way says so
 * instead of cancelling and starting again. What the agent said and what a person typed are
 * drawn as turns; the shell commands and status changes between them stay as thin activity
 * lines, so the conversation is readable at a glance and the work is still all there. The
 * questions are the agent's own JSON Schema rendered by the Portal's form stack, so an answer
 * is validated against the schema that asked for it before it is posted (AG-45, UI-37).
 */
/** How often the wait is counted, and how long a wait is long enough to say something about. */
const STALL_TICK_SECONDS = 5;
const STALL_AFTER_SECONDS = 45;

/**
 * How long the run has been quiet, once that is long enough to be worth saying.
 *
 * An answer that never arrives looks exactly like one still being written: `progressOf` reports
 * "working" whatever the elapsed time, so the transcript sat there and the person had nothing to
 * read and nothing to press (T-1761).
 */
function StallNotice({
  working,
  onCancel,
  onRetry,
}: {
  working: boolean;
  onCancel?: () => void;
  onRetry?: () => void;
}): JSX.Element | null {
  const { t } = useTranslation();
  const [waited, setWaited] = useState(0);
  useEffect(() => {
    if (!working) return;
    const tick = setInterval(
      () => setWaited((seconds) => seconds + STALL_TICK_SECONDS),
      STALL_TICK_SECONDS * 1000,
    );
    return () => clearInterval(tick);
  }, [working]);
  if (!working || waited < STALL_AFTER_SECONDS) return null;
  return (
    <Alert
      tone="info"
      className="mt-2"
      actions={
        onRetry || onCancel ? (
          <>
            {/* A run that died answers nothing, whatever it is told: asking again in a fresh
                conversation is the way on, and stopping this one is the way out (T-2462). */}
            {onRetry ? (
              <Button variant="primary" size="xs" onClick={onRetry}>
                {t("agentRun.conversation.retry")}
              </Button>
            ) : null}
            {onCancel ? (
              <Button variant="secondary" size="xs" onClick={onCancel}>
                {t("agentRun.conversation.stop")}
              </Button>
            ) : null}
          </>
        ) : undefined
      }
    >
      {t("agentRun.conversation.stalled", { seconds: waited })}
    </Alert>
  );
}

export function ConversationPanel({
  project,
  events,
  streaming,
  answering,
  sending,
  live,
  onAnswer,
  onSend,
  onCancel,
  onRetry,
  onNewConversation,
  attach,
  above,
  onUseEndpoint,
  usedEndpoints,
  building = false,
  onOpenLink,
  onBuildStarted,
}: {
  /** The project the run belongs to: what a card's links open. */
  project: string;
  events: RunEvent[];
  streaming: boolean;
  answering: boolean;
  sending: boolean;
  /** Whether the run still reads: a finished one takes no more instructions. */
  live: boolean;
  onAnswer: (questionId: string, answers: unknown) => void;
  /**
   * Takes the message. A promise is awaited: the box is emptied when the send succeeded and
   * keeps every word when it did not, with the failure shown beside it (T-1761).
   */
  onSend: (text: string) => void | Promise<unknown>;
  /** Stops a run that has stopped answering, offered once the wait is long enough. */
  onCancel?: () => void;
  /** Asks the newest question again in a fresh conversation, offered beside the stop (T-2462). */
  onRetry?: () => void;
  /** Leaves this conversation for an empty one: the way on once a run has ended (T-2463). */
  onNewConversation?: () => void;
  /** A control beside the text box, such as the dock's attach button. */
  attach?: ReactNode;
  /** A row above the text box, such as the dock's data bar. */
  above?: ReactNode;
  /** Adds an endpoint the catalog search found to the conversation's data (AG-75). */
  onUseEndpoint?: (name: string) => void;
  /**
   * Whether the run is building an application. A catalog search on the way to a build is a
   * step, not an answer, so its cards stay folded: the person asked for an application, not for
   * what the catalogue holds (T-0703).
   */
  building?: boolean;
  /** The endpoints the conversation queries, so a found one says it is in use. */
  usedEndpoints?: string[];
  /** Opens a Portal link of an answer in place, keeping the conversation (T-2773). */
  onOpenLink?: OpenLink;
  /** Follows the run the person started from a Build an app card (T-2721). */
  onBuildStarted?: (runId: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const [draft, setDraft] = useState("");
  const progress = progressOf(events);
  const answered = answeredSearches(events);
  const notices = noticesOf(events);
  const retried = retriedSteps(events);
  const newestRetry = Math.max(...retried);
  const echoed = echoedLines(events, notices);
  const questions = openQuestions(events);
  const withdrawn = withdrawnQuestions(events);
  const titles = new Map(
    events
      .filter((event) => event.kind === "question")
      .flatMap((event) => choicesOf(event.payload.schema as JsonSchema)?.choices ?? [])
      .map((choice) => [choice.value, choice.title] as const),
  );
  const foot = useRef<HTMLDivElement>(null);
  const [failed, setFailed] = useState<string | null>(null);
  // How long the newest turn has been the newest one. An answer that never comes is otherwise
  // indistinguishable from one still being written: the progress line says "working" either way.
  // The clock restarts during the render that brings a new turn, which is React's own way of
  // adjusting state to a prop, rather than in an effect that would render the stale value first.
  // Only while the agent owes the answer. A run that has answered and waits for the person is
  // quiet because it is their turn: counting that as a stall told the owner "no answer for 55
  // seconds" under an answer, and the one button offered stopped the conversation (T-2461).
  const working = answering || progress === "working";

  // A chat that does not follow its own newest line is a log. `block: "nearest"` keeps the
  // scrolling inside the transcript rather than dragging the whole page; jsdom has no such
  // method at all, which is why it is called only where it exists.
  useEffect(() => {
    foot.current?.scrollIntoView?.({ block: "nearest" });
  }, [events.length]);

  // A turn that ended without an answer (T-2772): its reason is the newest line, and the way on
  // is the same message sent again in this conversation, one press.
  const newest = events.length > 0 ? events[events.length - 1] : undefined;
  const newestWords = [...events]
    .reverse()
    .find((event) => event.kind === "message" && event.payload.sentBy !== "agent" && typeof event.payload.text === "string")
    ?.payload.text as string | undefined;
  const unanswered =
    newest?.kind === "thought" && newest.payload.failed === true
      ? newestWords
      : undefined;
  const endedBecause = [...events]
    .reverse()
    .find((event) => event.kind === "status" && typeof event.payload.reason === "string" && event.payload.reason !== "")
    ?.payload.reason as string | undefined;
  const sendAgain = (text: string): void => {
    setFailed(null);
    const answer = onSend(text);
    if (answer && typeof (answer as Promise<unknown>).then === "function") {
      void (answer as Promise<unknown>).catch((error: unknown) =>
        setFailed(error instanceof Error ? error.message : t("app.error.generic")),
      );
    }
  };

  const send = (): void => {
    const text = draft.trim();
    if (text === "" || sending) {
      return;
    }
    setFailed(null);
    const answer = onSend(text);
    if (answer && typeof (answer as Promise<unknown>).then === "function") {
      void (answer as Promise<unknown>).then(
        () => setDraft(""),
        // The message did not leave, and the box used to be empty by then: the person saw
        // nothing sent, nothing said, and nothing left to press send on again (T-1761).
        (error: unknown) =>
          setFailed(error instanceof Error ? error.message : t("app.error.generic")),
      );
      return;
    }
    setDraft("");
  };

  return (
    <section
      aria-labelledby="run-conversation"
      className="flex min-h-96 flex-col rounded border border-border"
    >
      <div className="flex items-baseline justify-between border-b border-border px-4 py-2">
        <h2 id="run-conversation" className="text-base font-semibold">
          {t("agentRun.conversation.title")}
        </h2>
        <span className="text-xs text-fg-muted" role="status">
          {streaming ? t("agentRun.conversation.live") : t("agentRun.conversation.offline")}
        </span>
      </div>

      {/* UI-39: a conversation that only appears is a conversation a screen reader misses. The
          log announces what arrives, politely and by itself — `aria-atomic="false"` so the
          reader hears the new turn and not the whole history again (T-1052). */}
      <div
        role="log"
        aria-live="polite"
        aria-atomic="false"
        className="min-h-0 flex-1 overflow-y-auto px-4 py-3"
      >
        {events.length === 0 && (
          <EmptyState
            bare
            title={t("agentRun.conversation.empty")}
            description={t("agentRun.conversation.emptyHint")}
            icon="chat"
          />
        )}

        <ol className="space-y-2 text-sm" aria-label={t("agentRun.conversation.title")}>
          {rowsOf(events).map((row) => {
            if ("details" in row) {
              const [only] = row.details;
              // One line behind a toggle named "Details (1 line)" says nothing: the line itself
              // does (T-2769). A preview stays a link, behind the toggle as before.
              if (row.details.length === 1 && only.kind !== "preview") {
                return (
                  <li key={`details-${only.seq}`} className="text-xs text-fg-muted">
                    {line(only, t)}
                  </li>
                );
              }
              return (
                <li key={`details-${row.details[0].seq}`} className="text-xs text-fg-muted">
                  <details>
                    <summary className="cursor-pointer">
                      {t("agentRun.conversation.details", { count: row.details.length })}
                    </summary>
                    <div className="mt-1 space-y-0.5 font-mono">
                      {row.details.map((event) => {
                        const preview =
                          event.kind === "preview" && typeof event.payload.previewUrl === "string"
                            ? event.payload.previewUrl
                            : null;
                        return (
                          <p key={event.seq} className="break-words">
                            {preview !== null ? (
                              // The preview URL is written by the workspace, the least trusted
                              // writer this file names. It went into `href` unchecked, so a
                              // `javascript:` preview ran on the Portal's own origin, with the
                              // reader's session, the moment they clicked it (PF-50).
                              <ExternalLink
                                className="focus-ring rounded-sm text-primary-soft-fg underline"
                                href={preview}
                              >
                                {t("agentRun.line.previewLink")}
                              </ExternalLink>
                            ) : (
                              line(event, t)
                            )}
                          </p>
                        );
                      })}
                    </div>
                  </details>
                </li>
              );
            }
            const { event, count } = row;
            if (echoed.has(event.seq)) {
              return null;
            }
            const speaker = speakerOf(event.kind);
            if (event.kind === "tool") {
              // What the assistant found is drawn as cards above the step itself (UI-46), when
              // its answer was about what it found.
              const found =
                event.payload.tool === "search_catalog" && answered.has(event.seq) && !building
                  ? catalogItemsOf(event.payload.output)
                  : null;
              const proposal =
                event.payload.tool === "propose_endpoint" ? proposalOf(event.payload.output) : null;
              const kpi =
                event.payload.tool === "compute_kpi" ? kpiOf(event.payload.output, event.payload.input) : null;
              const queried = queryResultOf(event.payload, i18n.resolvedLanguage ?? i18n.language);
              const kpiPipeline =
                event.payload.tool === "draft_kpi_pipeline" ? kpiPipelineOf(event.payload.output) : null;
              const tested = changeTestOf(event.payload);
              const write = entityWriteOf(event.payload);
              // A Change the assistant proposed is followed here to a result that works (T-2774).
              const followed = followedChangeOf(event.payload);
              const notice = notices.get(event.seq);
              return (
                <li key={event.seq} className="space-y-2">
                  {found !== null ? (
                    <CatalogCards
                      project={project}
                      items={found}
                      datasets={catalogDatasetsOf(event.payload.output)}
                      onUseEndpoint={live ? onUseEndpoint : undefined}
                      usedEndpoints={usedEndpoints}
                      words={wordsOf(event.payload.input)}
                    />
                  ) : null}
                  {proposal !== null ? (
                    <EndpointProposalCard project={project} proposal={proposal} />
                  ) : null}
                  {kpi !== null ? <KpiCard project={project} kpi={kpi} onSend={live ? onSend : undefined} /> : null}
                  {kpiPipeline !== null ? <KpiPipelineCard project={project} pipeline={kpiPipeline} /> : null}
                  {tested !== null ? <ChangeTestCard test={tested} /> : null}
                  {write !== null ? <EntityWriteCard write={write} live={live} /> : null}
                  {followed !== null ? (
                    <ChangeFollowCard project={project} followed={followed} onOpenLink={onOpenLink} />
                  ) : null}
                  {queried !== null ? <QueryResultCard result={queried} project={project} onAsk={live ? sendAgain : undefined} /> : null}
                  <ActionStep
                    event={event}
                    live={live}
                    onSend={onSend}
                    count={count}
                    noticed={notice !== undefined || (retried.has(event.seq) && event.seq !== newestRetry)}
                    retried={retried.has(event.seq)}
                  />
                  {notice !== undefined ? (
                    <StepNotice
                      kind={notice}
                      project={project}
                      words={newestWords}
                      onAsk={live ? sendAgain : undefined}
                      onOpenLink={onOpenLink}
                    />
                  ) : null}
                </li>
              );
            }
            if (event.kind === "app-build") {
              // What Build an app answered in the chat, and the person's own Build (T-2721).
              const build = appBuildOf(event.payload);
              return build === null ? null : (
                <li key={event.seq}>
                  <AppBuildCard project={project} build={build} onStarted={onBuildStarted} />
                </li>
              );
            }
            if (speaker === "activity") {
              const text = line(event, t);
              return text === "" ? null : (
                <li key={event.seq} data-testid={`${event.kind}-line`} className="flex items-center gap-1.5 text-xs text-fg-muted">
                  <span aria-hidden className="h-px flex-1 bg-border" />
                  <span className="min-w-0 break-words">{text}</span>
                  <span aria-hidden className="h-px flex-1 bg-border" />
                </li>
              );
            }
            const mine = speaker === "person";
            // The words while the model still writes them: seen, and kept out of the log's
            // announcements, so a screen reader hears the finished answer once rather than every
            // half of it (UI-39, T-2821).
            const writing = event.kind === "partial";
            return (
              <li
                key={event.seq}
                className={mine ? "flex justify-end" : "flex"}
                aria-hidden={writing ? true : undefined}
                data-testid={writing ? "partial-line" : undefined}
              >
                <div className="min-w-0 max-w-[85%]">
                  <p className={mine ? "text-right text-xs text-fg-muted" : "text-xs text-fg-muted"}>
                    {labelOf(event, t)}
                  </p>
                  {mine ? (
                    // What the person wrote stays as written.
                    <p className="mt-0.5 whitespace-pre-wrap break-words rounded-lg rounded-br-sm bg-primary px-3 py-2 text-primary-fg">
                      {line(event, t, titles)}
                    </p>
                  ) : event.kind === "thought" || writing ? (
                    // An answer is Markdown, drawn as its lists, tables and links; it loses its
                    // "(AG-70)" (T-2756, T-2773).
                    <div className="mt-0.5 rounded-lg rounded-bl-sm bg-surface-subtle px-3 py-2">
                      <Prose text={forPeople(line(event, t, titles))} onOpenLink={onOpenLink} />
                    </div>
                  ) : (
                    <p
                      className={clsx(
                        "mt-0.5 whitespace-pre-wrap break-words rounded-lg rounded-bl-sm bg-surface-subtle px-3 py-2",
                        event.kind === "question" &&
                          withdrawn.has(String(event.payload.questionId ?? "")) &&
                          "text-fg-muted line-through decoration-fg-muted/50",
                      )}
                    >
                      {forPeople(line(event, t, titles))}
                    </p>
                  )}
                  {event.kind === "question" && withdrawn.has(String(event.payload.questionId ?? "")) ? (
                    // The person wrote instead of answering: the form is gone, and this says why (T-6104).
                    <p className="mt-1 text-xs italic text-fg-muted">{t("agentRun.line.withdrawn")}</p>
                  ) : null}
                </div>
              </li>
            );
          })}
        </ol>
        {progress !== null ? (
          <p data-testid="run-progress" role="status" className="mt-2 text-xs text-fg-muted">
            {t(`agentRun.progress.${progress}`)}
          </p>
        ) : null}

        {/* A stream that dies mid-answer used to be one grey word changing from "live" to
            "offline" in the header, while half-written answers stayed on screen looking
            finished (T-1761). */}
        {live && !streaming ? (
          <Alert tone="warning" className="mt-2">
            {t("agentRun.conversation.dropped")}
          </Alert>
        ) : null}

        {/* Keyed on the newest turn: a turn that arrives remounts the wait and starts it over,
            which is the reset without a clock read during the render (T-1761). */}
        <StallNotice key={events.length} working={working} onCancel={onCancel} onRetry={onRetry} />

        {live && unanswered !== undefined && unanswered.trim() !== "" ? (
          <Alert
            tone="warning"
            className="mt-2"
            actions={
              <Button variant="primary" size="xs" disabled={sending} disabledReason={t("app.busyReason")} onClick={() => sendAgain(unanswered)}>
                {t("agentRun.conversation.tryAgain")}
              </Button>
            }
          >
            {t("agentRun.conversation.unanswered")}
          </Alert>
        ) : null}

        {questions.map((question) => (
          <div
            key={question.questionId}
            className="mt-3 rounded-lg border border-border bg-surface-subtle p-3"
          >
            <h3 className="text-sm font-semibold">{t("agentRun.conversation.question")}</h3>
            <QuestionOptions
              schema={question.schema as JsonSchema}
              disabled={answering}
              free={!question.pick}
              disabledReasons={question.disabledReasons}
              onAnswer={(data) => {
                onAnswer(question.questionId, data);
              }}
            />
            {question.input ? (
              <QuestionData
                input={question.input}
                disabled={answering}
                onAnswer={(data) => {
                  onAnswer(question.questionId, data);
                }}
              />
            ) : null}
          </div>
        ))}

        <div ref={foot} />
      </div>

      {live ? (
        <form
          className="flex flex-col gap-2 border-t border-border p-3"
          onSubmit={(event) => {
            event.preventDefault();
            send();
          }}
        >
          {above}
          {failed ? (
            <Alert tone="danger" className="mb-1">
              {t("agentRun.conversation.sendFailed", { reason: failed })}
            </Alert>
          ) : null}
          <div className="flex items-end gap-2">
            {attach}
            <label className="sr-only" htmlFor="run-message">
              {t("agentRun.conversation.placeholder")}
            </label>
            <Textarea
              id="run-message"
              rows={3}
              value={draft}
              placeholder={t("agentRun.conversation.placeholder")}
              onChange={(event) => {
                setDraft(event.target.value);
              }}
              // Enter sends, as every chat box does; a newline is still Shift+Enter.
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  send();
                }
              }}
              className="min-h-0 flex-1 resize-none"
            />
            <Button
              type="submit"
              variant="primary"
              disabled={draft.trim() === "" || sending}
              disabledReason={sending ? t("app.busyReason") : t("agentRun.conversation.writeFirst")}
            >
              {t("agentRun.conversation.send")}
            </Button>
          </div>
        </form>
      ) : (
        <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border p-3">
          <div className="text-sm text-fg-muted">
            <p>{t("agentRun.conversation.closed")}</p>
            {/* Who or what ended it, as the Portal said on the status (T-2772): a conversation
                that says only "cancelled" between two steps leaves the person guessing. */}
            {endedBecause !== undefined ? (
              <p data-testid="ended-because">{t("agentRun.conversation.endedBecause", { reason: endedBecause })}</p>
            ) : null}
          </div>
          {/* An ended run reads nothing more; without this the only way back to a working chat
              was the dock's close button, which nobody reads as "start again" (T-2463). */}
          {onNewConversation ? (
            <Button variant="primary" size="sm" onClick={onNewConversation}>
              {t("agentRun.conversation.newConversation")}
            </Button>
          ) : null}
        </div>
      )}
    </section>
  );
}
