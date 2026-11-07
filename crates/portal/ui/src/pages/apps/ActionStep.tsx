import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Alert, RecordView, Button } from "../../components/ui";
import { PortalLink } from "./Prose";
import type { OpenLink } from "./Prose";
import type { RunEvent } from "./useAgentRun";
import { viewOf } from "./QueryResultCard";

/**
 * What a person reads about a failed assistant step. The wording is the Portal's own, picked by
 * the kind of failure: the tool's error is written for the model, and it quotes arguments, field
 * names and whole answers of the systems behind the step.
 */
export type FailureKind = "argument" | "refused" | "guarded" | "page" | "missing" | "test" | "other";

/** First match wins: a refusal may also quote a status or a missing name. */
const FAILURES: [FailureKind, RegExp][] = [
  ["guarded", /never an instruction/i],
  ["refused", /forbidden|access denied|not permitted|you may read no|unauthori[sz]ed|"status":\s*40[13]/i],
  ["test", /test is not green/i],
  ["missing", /not found|does not exist|is not in this project|is not an entity type|"status":\s*404/i],
  [
    "argument",
    /argument|names no |does not parse|not a valid|invalid input|bad request|is not one of|changes nothing|stays as it is|does not use|"status":\s*400/i,
  ],
];

/** Tools that open a page: their failure is about the page, whatever the words. */
const NAVIGATING = new Set(["navigate", "jc_ui_navigate"]);

/** The kind of a failed assistant step; `null` for a step that did not fail or is a builder's. */
export function failureOf(payload: Record<string, unknown>): FailureKind | null {
  const tool = typeof payload.tool === "string" ? payload.tool : "";
  if (!isAssistantTool(tool) || !didFail(payload)) {
    return null;
  }
  if (NAVIGATING.has(tool)) {
    return "page";
  }
  const error = typeof payload.error === "string" ? payload.error : JSON.stringify(payload.error ?? "");
  return FAILURES.find(([, words]) => words.test(error))?.[0] ?? "other";
}

/** Whatever could hold a credential or a caller's query in a text a person sees or copies. */
export function scrubbed(text: string): string {
  return text
    .replace(/\b(bearer|basic|token)[ =:]+[\w.~+/=-]{8,}/gi, "$1 …")
    .replace(/\beyJ[\w-]+\.[\w-]+(\.[\w-]+)?/g, "…")
    .replace(/:\/\/[^/\s@"'`]+@/g, "://")
    .replace(/(https?:\/\/[^\s?#"'`]+)[?#][^\s"'`]*/g, "$1");
}

/** The failures a transcript answers with a state of its own instead of its small step line. */
const NOTICED = new Set<FailureKind>(["refused", "guarded", "page", "missing"]);

/**
 * The steps that end in a notice, by `seq`. A step the assistant then got right was only its way
 * to the answer, and each kind says its next step once, on the newest failure.
 */
export function noticesOf(events: RunEvent[]): Map<number, FailureKind> {
  const standing = new Map<FailureKind, { seq: number; tool: unknown }>();
  for (const event of events) {
    if (event.kind !== "tool") {
      continue;
    }
    const kind = failureOf(event.payload);
    if (kind === null) {
      for (const [failed, step] of standing) {
        if (step.tool === event.payload.tool) {
          standing.delete(failed);
        }
      }
    } else if (NOTICED.has(kind)) {
      standing.set(kind, { seq: event.seq, tool: event.payload.tool });
    }
  }
  return new Map([...standing].map(([kind, { seq }]) => [seq, kind]));
}

/**
 * The slips the assistant went past: a later step worked or it tried again. Those are its way to
 * the answer, so they draw as tries, not as failures above a good result.
 */
export function retriedSteps(events: RunEvent[]): Set<number> {
  const steps = events.filter((event) => event.kind === "tool");
  return new Set(
    steps
      .filter((step, at) => failureOf(step.payload) === "argument" && at < steps.length - 1)
      .map((step) => step.seq),
  );
}

/** The opening of the sentence the Portal itself ends such a turn with, by the state saying it. */
const NOTICE_SAYS: [FailureKind, string][] = [
  ["page", "That page could not be opened."],
  ["guarded", "Nothing was changed: this step came from the data"],
];

/** The Portal's own closing lines a notice above already says: one of the two is enough. */
export function echoedLines(events: RunEvent[], notices: Map<number, FailureKind>): Set<number> {
  const standing = new Set(notices.values());
  return new Set(
    events
      .filter(
        ({ kind, payload }) =>
          kind === "thought" &&
          NOTICE_SAYS.some(([by, opening]) => standing.has(by) && String(payload.text ?? "").startsWith(opening)),
      )
      .map((event) => event.seq),
  );
}

/** The pages a person is offered when the one asked for would not open. */
const PAGES = ["spaces", "models", "endpoints", "explore"];

/** A failure drawn as a state: what happened and what the person can do next. */
export function StepNotice({
  kind,
  project,
  words,
  onAsk,
  onOpenLink,
}: {
  kind: FailureKind;
  project: string;
  /** The person's own newest words, which can carry a stopped step when sent again. */
  words?: string;
  /** Sends a message for the person: the one press a notice offers. */
  onAsk?: (text: string) => void;
  onOpenLink?: OpenLink;
}): JSX.Element {
  const { t } = useTranslation();
  const ask = kind === "guarded" ? words : kind === "missing" ? t("agentRun.failure.holds") : undefined;
  return (
    <Alert
      data-testid="step-notice"
      tone={kind === "refused" ? "warning" : "info"}
      title={t(`agentRun.failure.${kind}`)}
      actions={
        ask !== undefined && onAsk ? (
          <Button size="sm" variant="secondary" onClick={() => onAsk(ask)}>
            {t(kind === "guarded" ? "agentRun.failure.askAgain" : "agentRun.failure.askHolds")}
          </Button>
        ) : undefined
      }
    >
      <p>{t(`agentRun.failure.next.${kind}`)}</p>
      {kind === "page" ? (
        <ul className="mt-1 flex flex-wrap gap-x-3 gap-y-1">
          {PAGES.map((page) => (
            <li key={page}>
              <PortalLink href={`/projects/${encodeURIComponent(project)}/${page}`} onOpenLink={onOpenLink}>
                {t(`agentRun.failure.page_${page}`)}
              </PortalLink>
            </li>
          ))}
        </ul>
      ) : null}
    </Alert>
  );
}

/**
 * Why a step failed, on one line, or "" when the step says nothing: a plain error, a runtime
 * error with the file and line it was thrown at, or the blocks a patch could not apply. The
 * transcript shows it on the step's own row, so a failure is readable without opening it.
 */
export function errorText(
  payload: Record<string, unknown>,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  const { error, refused } = payload;
  if (typeof error === "string") {
    return scrubbed(error);
  }
  if (typeof error === "object" && error !== null) {
    const { message, file, line } = error as Record<string, unknown>;
    if (typeof message === "string" && message !== "") {
      return scrubbed(
        typeof file === "string" && file !== "" && line !== undefined && line !== null
          ? `${message} (${file}:${String(line)})`
          : message,
      );
    }
  }
  if (Array.isArray(refused) && refused.length > 0) {
    const first = refused[0] as Record<string, unknown> | null;
    const reason = typeof first?.reason === "string" ? first.reason : "";
    const path = typeof first?.path === "string" && first.path !== "" ? first.path : "";
    return t("agentRun.step.refusedLine", {
      count: refused.length,
      reason: scrubbed(path !== "" && reason !== "" ? `${path}: ${reason}` : reason || path),
    });
  }
  return "";
}

/** The assistant's own tools, named as what they did; a shell step keeps its tool's name. */
const LABELLED = new Set([
  "search_catalog",
  "query_endpoint",
  "change_resource",
  "write_entities",
  "grant_role",
  "propose_endpoint",
  "edit_endpoint",
  "space_complete",
  "profile_sample",
  "compute_kpi",
  "draft_kpi_pipeline",
  "navigate",
  "jc_ui_navigate",
  "describe_tool",
]);

/** A step of the assistant's own, as opposed to the builder's shell and file steps. */
function isAssistantTool(tool: string): boolean {
  return LABELLED.has(tool) || tool.startsWith("jc_");
}

function didFail(payload: Record<string, unknown>): boolean {
  const { status, error, exitCode } = payload;
  return status === "failed" || error !== undefined || (typeof exitCode === "number" && exitCode !== 0);
}

function labelOf(
  tool: string,
  payload: Record<string, unknown>,
  t: (key: string, options?: Record<string, unknown>) => string,
): string | null {
  if (!isAssistantTool(tool)) {
    return null;
  }
  if (!LABELLED.has(tool)) {
    return t("agentRun.step.label.portal");
  }
  const input = payload.input as { endpoint?: unknown; arguments?: { type?: unknown } } | null | undefined;
  const endpoint = input?.endpoint;
  if (tool !== "query_endpoint" || typeof endpoint !== "string" || endpoint === "") {
    return t(`agentRun.step.label.${tool}`);
  }
  // What was read, by the count and the type, not only where from (T-2769).
  const view = payload.status === "failed" || payload.output === undefined ? null : viewOf(payload.output);
  if (view?.kind !== "table") {
    return t("agentRun.step.label.readEndpoint", { endpoint });
  }
  const asked = input?.arguments?.type;
  const type = typeof asked === "string" && asked !== "" ? asked : t("agentRun.step.label.entities");
  return view.total > view.rows.length
    ? t("agentRun.step.label.readSome", { shown: view.rows.length, total: view.total, type, endpoint })
    : t("agentRun.step.label.readAll", { count: view.total, type, endpoint });
}

/**
 * One step the assistant took, inspectable (AG-56, OPS-50).
 *
 * The summary is the line the transcript always showed; open, the step shows what the tool was
 * given, what it produced or why it failed, and the change it made. Every value comes from the
 * workspace and is rendered as text, never as markup (AG-46): a `<script>` in an output is a
 * `<script>` on the screen and nothing else. A failed step can be sent back into the run with
 * one click, and every step hands its trace over as JSON.
 */
export function ActionStep({
  event,
  live,
  onSend,
  count = 1,
  noticed = false,
  retried = false,
}: {
  event: RunEvent;
  live: boolean;
  onSend: (text: string) => void;
  /** How many times in a row the same step repeated; the event is the newest of them. */
  count?: number;
  /** Another line says why (a `StepNotice`, a newer try), so the step's own row need not. */
  noticed?: boolean;
  /** The assistant went past this step: it draws as a try, not as a failure. */
  retried?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const payload = event.payload;
  const tool = typeof payload.tool === "string" && payload.tool !== "" ? payload.tool : "tool";
  const exitCode = typeof payload.exitCode === "number" ? payload.exitCode : undefined;
  const failed = didFail(payload);
  const duration = typeof payload.durationMs === "number" ? payload.durationMs : undefined;
  const label = labelOf(tool, payload, t);
  const failure = failureOf(payload);
  const reason = failed ? errorText(payload, t) : "";
  const said = noticed ? "" : retried ? t("agentRun.step.retried") : failure !== null ? t(`agentRun.failure.${failure}`) : reason;
  const error = payload.error === undefined ? undefined : reason;
  // An MCP refusal's output is its error again, unscrubbed.
  const output = (payload.output as { isError?: unknown } | null | undefined)?.isError === true ? undefined : payload.output;
  const refused = Array.isArray(payload.refused) && payload.refused.length > 0 ? payload.refused : undefined;
  const sections: Array<[string, unknown]> = [
    [t("agentRun.step.input"), payload.input ?? payload.command],
    [t("agentRun.step.output"), output],
    [t("agentRun.step.error"), error],
    [t("agentRun.step.refused"), refused],
    [t("agentRun.step.diff"), payload.diff],
  ];

  const copy = async (): Promise<void> => {
    const trace = {
      seq: event.seq,
      tool,
      status: failed ? "failed" : "ok",
      durationMs: duration,
      input: payload.input ?? payload.command,
      output,
      error,
      diff: payload.diff,
    };
    try {
      await navigator.clipboard.writeText(JSON.stringify(trace, null, 2));
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <details className="rounded border border-border bg-surface-subtle text-xs">
      <summary className="flex cursor-pointer flex-wrap items-center gap-x-2 px-2 py-1 font-mono">
        <span
          role="img"
          aria-label={t(retried ? "agentRun.step.retried" : failed ? "agentRun.step.failed" : "agentRun.step.ok")}
          className={retried ? "text-fg-muted" : failed ? "text-danger" : "text-success"}
        >
          {retried ? "↻" : failed ? "✗" : "✓"}
        </span>
        {/* A flex item that may break words shrinks to one letter per line in the dock's narrow
            column (T-0892), so the label keeps a least width and the row wraps. The reason takes a
            row of its own: a cut sentence tells a person nothing. */}
        {label !== null ? (
          <span className="min-w-32 flex-1 font-sans [overflow-wrap:anywhere]" title={label}>
            {label}
          </span>
        ) : (
          <span className="min-w-0 flex-1 truncate" title={tool}>
            {tool}
          </span>
        )}
        {count > 1 ? (
          <span data-testid="step-count" className="rounded bg-surface px-1 text-fg-muted">
            ×{count}
          </span>
        ) : null}
        {/* Under a millisecond is no time a person reads: "0 ms" looked like nothing done (T-2763). */}
        {duration !== undefined && duration > 0 ? (
          <span className="shrink-0 text-fg-muted">{t("agentRun.step.duration", { ms: duration })}</span>
        ) : null}
        {said !== "" ? (
          <span data-testid="step-reason" title={said} className={`line-clamp-3 min-w-0 basis-full font-sans ${retried || failure === "argument" ? "text-fg-muted" : "text-danger"}`}>
            {said}
          </span>
        ) : null}
      </summary>
      <div className="space-y-2 px-2 pb-2">
        {sections.map(([label, value]) =>
          value === undefined || value === null || value === "" ? null : (
            <div key={label}>
              <p className="font-medium text-fg-muted">{label}</p>
              <RecordView value={value} label={label} className="mt-0.5 max-h-64 rounded bg-surface p-2" />
            </div>
          ),
        )}
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            onClick={() => {
              void copy();
            }}
          >
            {copied ? t("agentRun.step.copied") : t("agentRun.step.copy")}
          </Button>
          {failed && live ? (
            <Button
              size="sm"
              onClick={() => {
                // The assistant holds its own step's error: the person's bubble needs none of it.
                onSend(
                  failure !== null
                    ? t("agentRun.step.fixPlain", { step: label ?? "" })
                    : t("agentRun.step.fixMessage", {
                        tool,
                        error: reason !== "" ? reason : String(exitCode ?? ""),
                      }),
                );
              }}
            >
              {t("agentRun.step.fix")}
            </Button>
          ) : null}
        </div>
      </div>
    </details>
  );
}
