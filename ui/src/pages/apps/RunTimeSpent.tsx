import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import type { RunEvent } from "./useAgentRun";
import { Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "../../components/ui";

/** One step of a run that took time: a model call, a tool, or the person answering a question. */
export interface TimedStep {
  /** Milliseconds from the run's first event to the step's start; null when the event is older than its timestamp. */
  at: number | null;
  kind: "model" | "tool" | "waiting";
  /** The tool's name or the model's; empty for waiting. */
  name: string;
  ms: number;
  failed: boolean;
}

export interface TimeSpent {
  model: number;
  tools: number;
  waiting: number;
  steps: TimedStep[];
}

function instant(event: RunEvent): number | null {
  const stamp = event.payload.timestamp;
  if (typeof stamp !== "string") {
    return null;
  }
  const parsed = Date.parse(stamp);
  return Number.isNaN(parsed) ? null : parsed;
}

function millis(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

/**
 * Where a run's seconds went (T-2771), read from its events alone: each `usage` frame is one
 * model call with its `latencyMs`, each `tool` step its `durationMs`, and a `question` until the
 * `answer` after it is the person's time. An event written before frames carried `timestamp`
 * still counts toward the totals, only without its start.
 */
export function timeSpent(events: RunEvent[]): TimeSpent {
  const stamps = events.map(instant).filter((at): at is number => at !== null);
  const start = stamps.length > 0 ? Math.min(...stamps) : null;
  const offset = (event: RunEvent): number | null => {
    const at = instant(event);
    return at === null || start === null ? null : at - start;
  };
  const spent: TimeSpent = { model: 0, tools: 0, waiting: 0, steps: [] };
  let asked: RunEvent | null = null;
  for (const event of events) {
    if (event.kind === "usage") {
      const ms = millis(event.payload.latencyMs);
      if (ms !== null) {
        spent.model += ms;
        const model = typeof event.payload.model === "string" ? event.payload.model : "";
        spent.steps.push({ at: offset(event), kind: "model", name: model, ms, failed: false });
      }
    } else if (event.kind === "tool") {
      const ms = millis(event.payload.durationMs);
      if (ms !== null) {
        spent.tools += ms;
        const tool = typeof event.payload.tool === "string" ? event.payload.tool : "";
        const failed = event.payload.status === "failed";
        spent.steps.push({ at: offset(event), kind: "tool", name: tool, ms, failed });
      }
    } else if (event.kind === "question") {
      asked = event;
    } else if (event.kind === "answer" && asked !== null) {
      const from = instant(asked);
      const to = instant(event);
      if (from !== null && to !== null && to >= from) {
        spent.waiting += to - from;
        spent.steps.push({ at: offset(asked), kind: "waiting", name: "", ms: to - from, failed: false });
      }
      asked = null;
    }
  }
  return spent;
}

/** A duration in the reader's language: milliseconds under a second, seconds under a minute. */
export function duration(ms: number, language: string): string {
  const format = (value: number, unit: string, digits: number): string =>
    new Intl.NumberFormat(language, { style: "unit", unit, unitDisplay: "short", maximumFractionDigits: digits }).format(value);
  if (ms < 1000) {
    return format(Math.round(ms), "millisecond", 0);
  }
  if (ms < 60_000) {
    return format(ms / 1000, "second", 1);
  }
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${format(minutes, "minute", 0)} ${format(seconds, "second", 0)}`;
}

/** The step's start as `+m:ss` from the run's first event. */
function started(at: number): string {
  const total = Math.round(at / 1000);
  return `+${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/**
 * The run's time, split into the model, the tools and the person, with every timed step in order
 * (T-2771): a slow answer is traced to its step here. Nothing is drawn for a run that recorded no
 * timings, so a run from before them reads as it did.
 */
export function RunTimeSpent({ events }: { events: RunEvent[] }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const spent = timeSpent(events);
  if (spent.steps.length === 0) {
    return null;
  }
  const language = i18n.language;
  const slowest = spent.steps.reduce((a, b) => (b.ms > a.ms ? b : a));
  const label = (step: TimedStep): string =>
    step.kind === "model"
      ? step.name
        ? t("agentRun.timeSpent.modelCallOf", { model: step.name })
        : t("agentRun.timeSpent.modelCall")
      : step.kind === "waiting"
        ? t("agentRun.timeSpent.waitingStep")
        : step.name;

  return (
    <section aria-labelledby="run-time-spent" className="rounded border border-border p-4">
      <h2 id="run-time-spent" className="text-base font-semibold">
        {t("agentRun.timeSpent.title")}
      </h2>
      <dl className="mt-2 flex flex-wrap gap-x-6 gap-y-1 text-body">
        {(
          [
            ["model", spent.model],
            ["tools", spent.tools],
            ["waiting", spent.waiting],
          ] as const
        ).map(([part, ms]) => (
          <div key={part} className="flex gap-2">
            <dt className="text-fg-muted">{t(`agentRun.timeSpent.${part}`)}</dt>
            <dd className="font-medium">{duration(ms, language)}</dd>
          </div>
        ))}
      </dl>
      <p className="mt-2 text-caption text-fg-muted">
        {t("agentRun.timeSpent.slowest", { step: label(slowest), took: duration(slowest.ms, language) })}
      </p>
      <div className="mt-2">
        <Table caption={t("agentRun.timeSpent.caption")} maxHeight="max-h-72">
          <TableHead>
            <TableHeaderCell>{t("agentRun.timeSpent.started")}</TableHeaderCell>
            <TableHeaderCell>{t("agentRun.timeSpent.step")}</TableHeaderCell>
            <TableHeaderCell align="right">{t("agentRun.timeSpent.took")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {spent.steps.map((step, index) => (
              <TableRow key={index}>
                <TableCell className="tabular-nums text-fg-muted">{step.at === null ? "" : started(step.at)}</TableCell>
                <TableCell>
                  {label(step)}
                  {step.failed && <span className="ml-2 text-danger">{t("agentRun.timeSpent.failed")}</span>}
                </TableCell>
                <TableCell align="right" className="tabular-nums">
                  {duration(step.ms, language)}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </section>
  );
}
