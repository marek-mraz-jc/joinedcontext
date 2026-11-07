import { PageFailed } from "../../components/ui/PageState";
import { urnName } from "../mqttbrokers/connections";
import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { usePermissions } from "../../api/permissions";
import type { components } from "../../api/schema";
import type { BadgeTone } from "../../components/ui/Badge";
import {
  Alert,
  Badge,
  Button,
  Dialog,
  EmptyState,
  Table,
  TableSkeleton,
} from "../../components/ui";

type Run = components["schemas"]["PipelineRun"];

/** What a run says beside its counts (T-3260): how long it took, and what moved since the run before. */
export interface RunFacts {
  /** Seconds from its first outcome to its last; `undefined` for a run with one instant. */
  seconds?: number;
  /** Written now minus written by the run before; `undefined` for the oldest run listed. */
  delta?: number;
  /** It wrote nothing while the run before wrote something: a feed or a mapping stopped. */
  silent: boolean;
}

/** The facts of every run of `runs`, newest first as the API lists them. */
export function runFacts(runs: Run[]): RunFacts[] {
  return runs.map((run, at) => {
    const before = runs[at + 1];
    const span = (Date.parse(run.lastAt) - Date.parse(run.firstAt)) / 1000;
    return {
      seconds: Number.isFinite(span) && span > 0 ? span : undefined,
      delta: before === undefined ? undefined : run.sent - before.sent,
      silent: before !== undefined && run.sent === 0 && before.sent > 0,
    };
  });
}
type LogLine = components["schemas"]["PipelineLogLine"];

const PAGE = 100;

const TONE: Record<LogLine["outcome"], BadgeTone> = {
  sent: "success",
  rejected: "warning",
  failed: "danger",
};

/**
 * A pipeline's latest runs with how many records each sent, how many the model rejected and how
 * many failed in a step, and the log of the run a person opens: one line per record, newest first
 * (PL-62, ADR-N-034). A run is one tick of the pipeline's clock, or one UTC hour for a source
 * that never ends. Record ids and messages arrive masked where they look like a credential.
 */
export function PipelineRunsDialog({
  project,
  name,
  onClose,
  initialRun,
  runsFromQueue = false,
}: {
  project: string;
  name: string;
  onClose: () => void;
  /** The run to open first, as a rejected record names it (T-3252). */
  initialRun?: string;
  /** The pipeline runs from the queue (PL-78), so a steward may start one run now (T-4349). */
  runsFromQueue?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Dialog
      open
      onOpenChange={(isOpen) => {
        if (!isOpen) {
          onClose();
        }
      }}
      title={t("pipelines.runs.title", { name })}
      description={t("pipelines.runs.lead")}
      size="xl"
      closeLabel={t("pipelines.runs.close")}
    >
      <PipelineRunsView project={project} name={name} initialRun={initialRun} runsFromQueue={runsFromQueue} />
    </Dialog>
  );
}

/** The runs table and the open run's log: in the dialog above and on a pipeline's own page (T-4817). */
export function PipelineRunsView({
  project,
  name,
  initialRun,
  runsFromQueue = false,
}: {
  project: string;
  name: string;
  initialRun?: string;
  runsFromQueue?: boolean;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const time = new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" });
  const [picked, setPicked] = useState<string | null>(initialRun ?? null);

  const runsKey = [...queryKeys.resource(project, "pipelines", name), "runs"];
  const runs = useQuery({
    queryKey: runsKey,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/runs", {
          params: { path: { project, name } },
        }),
      ),
  });
  const queryClient = useQueryClient();
  const permissions = usePermissions(project);
  const [started, setStarted] = useState<string | null>(null);
  const [runError, setRunError] = useState<string | null>(null);
  const runNow = useMutation({
    mutationFn: async () => {
      setRunError(null);
      setStarted(null);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/pipelines/{name}/runs", {
          params: { path: { project, name } },
        }),
      );
    },
    onSuccess: (result) => {
      setStarted(result.run);
      void queryClient.invalidateQueries({ queryKey: runsKey });
    },
    onError: (err) => {
      setRunError(err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic"));
    },
  });
  const runRefusal =
    !permissions.can("Pipeline", "propose") || !permissions.can("Pipeline", "approve")
      ? t("permissions.denied", { verb: "approve", kind: "Pipeline" })
      : undefined;
  const items: Run[] = runs.data?.items ?? [];
  const facts = runFacts(items);
  const took = (seconds: number | undefined) =>
    seconds === undefined
      ? "—"
      : seconds < 60
        ? t("pipelines.runs.seconds", { count: Math.round(seconds) })
        : t("pipelines.runs.minutes", { count: Math.round(seconds / 60) });
  // The newest run is open until a person picks another.
  const open = picked ?? items[0]?.run ?? null;

  const label = (run: string): string => {
    const at = new Date(run);
    if (Number.isNaN(at.getTime())) {
      return run;
    }
    // A run named by its hour is a source that never ends: the hour it read in.
    return /T\d{2}:00Z$/.test(run)
      ? t("pipelines.runs.hour", { at: time.format(at) })
      : time.format(at);
  };

  return (
    <div className="flex flex-col gap-4">
      {runsFromQueue ? (
        <div className="flex justify-end">
          <Button
            variant="primary"
            disabled={runRefusal !== undefined || runNow.isPending}
            disabledReason={runRefusal}
            onClick={() => runNow.mutate()}
          >
            {runNow.isPending ? t("pipelines.runs.starting") : t("pipelines.runs.runNow")}
          </Button>
        </div>
      ) : null}
      {started !== null ? (
        <Alert tone="success" role="status">
          {t("pipelines.runs.started", { run: label(started) })}
        </Alert>
      ) : null}
      {runError !== null ? (
        <Alert tone="danger" role="alert">
          {runError}
        </Alert>
      ) : null}
      {runs.isError ? (
        <PageFailed error={runs.error} onRetry={() => void runs.refetch()} />
      ) : runs.isPending ? (
        <Table caption={t("pipelines.runs.caption")} status={t("app.loading")}>
          <TableSkeleton columns={5} />
        </Table>
      ) : items.length === 0 ? (
        <EmptyState
          bare
          icon="pipelines"
          title={t("pipelines.runs.empty")}
          description={t("pipelines.runs.emptyHint")}
        />
      ) : (
        <>
          {facts[0]?.silent ? (
            <Alert tone="warning" role="status">
              {t("pipelines.runs.silent", { before: items[1].sent.toLocaleString(locale) })}
            </Alert>
          ) : null}
          <ManifestGrid<(typeof items)[number] & { fact: (typeof facts)[number] }>
            caption={t("pipelines.runs.caption")}
            storageKey="pipeline-runs"
            searchLabel={t("grid.searchIn", { list: t("pipelines.runs.caption") })}
            rows={items.map((run, at) => ({ ...run, fact: facts[at] }))}
            rowKey={(run) => run.run}
            query={runs}
            pageSize={20}
            name={{
              id: "run",
              label: t("pipelines.runs.run"),
              text: (run) => run.lastAt,
              cell: (run) => (
                <time dateTime={run.lastAt} className="whitespace-nowrap tabular-nums">
                  {label(run.run)}
                </time>
              ),
            }}
            columns={[
              {
                id: "sent",
                label: t("pipelines.runs.sent"),
                numeric: true,
                align: "right",
                text: (run) => String(run.sent),
                cell: (run) => (
                  <>
                    <span className="tabular-nums">{run.sent.toLocaleString(locale)}</span>
                    {run.created != null && run.updated != null && run.unchanged != null ? (
                      // What the written records did (T-3304); unknown for an older stream.
                      <span className="block text-caption text-fg-muted">
                        {t("pipelines.runs.split", { created: run.created, updated: run.updated, unchanged: run.unchanged })}
                      </span>
                    ) : null}
                  </>
                ),
              },
              {
                id: "rejected",
                label: t("pipelines.runs.rejected"),
                numeric: true,
                align: "right",
                text: (run) => String(run.rejected),
                cell: (run) => <span className={run.rejected > 0 ? "tabular-nums text-warning" : "tabular-nums"}>{run.rejected.toLocaleString(locale)}</span>,
              },
              {
                id: "failed",
                label: t("pipelines.runs.failed"),
                numeric: true,
                align: "right",
                text: (run) => String(run.failed),
                cell: (run) => <span className={run.failed > 0 ? "tabular-nums text-danger" : "tabular-nums"}>{run.failed.toLocaleString(locale)}</span>,
              },
              {
                id: "took",
                label: t("pipelines.runs.took"),
                numeric: true,
                align: "right",
                text: (run) => String(run.fact.seconds ?? ""),
                cell: (run) => <span className="whitespace-nowrap tabular-nums">{took(run.fact.seconds)}</span>,
              },
              {
                id: "delta",
                label: t("pipelines.runs.delta"),
                numeric: true,
                align: "right",
                text: (run) => String(run.fact.delta ?? ""),
                cell: (run) =>
                  run.fact.silent ? (
                    <Badge tone="warning">{t("pipelines.runs.silentBadge")}</Badge>
                  ) : run.fact.delta === undefined ? (
                    "—"
                  ) : (
                    <span className="tabular-nums">
                      {run.fact.delta === 0
                        ? t("pipelines.runs.same")
                        : `${run.fact.delta > 0 ? "+" : "−"}${Math.abs(run.fact.delta).toLocaleString(locale)}`}
                    </span>
                  ),
              },
              {
                id: "outcome",
                label: t("pipelines.runs.outcome"),
                filter: true,
                // A run's state for the filter: wrote nothing although the one before did, failed lines, rejected lines, or clean.
                text: (run) => (run.fact.silent ? "silent" : run.failed > 0 ? "failing" : run.rejected > 0 ? "rejected" : "clean"),
                valueLabel: (state) => t(`pipelines.runs.states.${state}`),
                cell: (run) => t(`pipelines.runs.states.${run.fact.silent ? "silent" : run.failed > 0 ? "failing" : run.rejected > 0 ? "rejected" : "clean"}`),
              },
            ]}
            actions={(run) => (
              <Button
                size="sm"
                aria-pressed={open === run.run}
                // One word in the cell; the run it opens is in the name a screen reader says.
                aria-label={t("pipelines.runs.show", { run: label(run.run) })}
                onClick={() => setPicked(run.run)}
              >
                {t("pipelines.runs.log")}
              </Button>
            )}
            empty={<EmptyState bare icon="pipelines" title={t("pipelines.runs.empty")} description={t("pipelines.runs.emptyHint")} />}
          />
          {open !== null ? (
            <RunLog
              key={open}
              project={project}
              name={name}
              run={open}
              title={label(open)}
              time={time}
              empty={items.some((run) => run.run === open && run.sent + run.rejected + run.failed === 0)}
            />
          ) : null}
        </>
      )}
    </div>
  );
}

/** One run's log, newest line first, a page at a time. */
function RunLog({
  project,
  name,
  run,
  title,
  time,
  empty,
}: {
  project: string;
  name: string;
  run: string;
  title: string;
  time: Intl.DateTimeFormat;
  /** The run read its source and had nothing to write, so it never had lines (T-3001). */
  empty: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  // The pages walked so far, as their `before`: the first is the newest page.
  const [cursors, setCursors] = useState<(number | undefined)[]>([undefined]);
  const before = cursors[cursors.length - 1];
  const page = useQuery({
    queryKey: [...queryKeys.resource(project, "pipelines", name), "runs", run, before ?? "newest"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/runs/{run}/log", {
          params: { path: { project, name, run }, query: { limit: PAGE, before } },
        }),
      ),
  });
  const caption = t("pipelines.runs.logCaption", { run: title });
  const lines: LogLine[] = page.data?.items ?? [];
  const withMessages = lines.some((line) => (line.message ?? "").trim() !== "");

  return (
    <section aria-label={caption} className="flex flex-col gap-2">
      <h3 className="text-body font-medium text-fg">{caption}</h3>
      {page.isError ? (
        <PageFailed error={page.error} onRetry={() => void page.refetch()} />
      ) : page.isPending ? (
        <Table caption={caption} status={t("app.loading")}>
          <TableSkeleton columns={4} />
        </Table>
      ) : lines.length === 0 ? (
        <p className="text-caption text-fg-subtle">
          {t(empty ? "pipelines.runs.nothingWritten" : "pipelines.runs.noLines")}
        </p>
      ) : (
        <>
          <ManifestGrid<LogLine>
            // A page whose lines carry messages starts with the Message column shown.
            key={String(withMessages)}
            caption={caption}
            storageKey="pipeline-run-log"
            searchLabel={t("grid.searchIn", { list: caption })}
            rows={lines}
            rowKey={(line) => String(line.id)}
            query={page}
            pageSize={PAGE}
            name={{
              id: "at",
              label: t("pipelines.runs.at"),
              text: (line) => line.at,
              cell: (line) => (
                <time dateTime={line.at} className="whitespace-nowrap tabular-nums">
                  {time.format(new Date(line.at))}
                </time>
              ),
            }}
            columns={[
              {
                id: "record",
                label: t("pipelines.runs.record"),
                text: (line) => line.recordId ?? "",
                cell: (line) =>
                  line.recordId ? <RecordId id={line.recordId} /> : <span className="text-caption text-fg-subtle">{t("pipelines.runs.noId")}</span>,
              },
              {
                id: "outcome",
                label: t("pipelines.runs.outcome"),
                filter: true,
                text: (line) => line.outcome,
                valueLabel: (outcome) => t(`pipelines.runs.outcomes.${outcome}`),
                cell: (line) => (
                  <div className="flex flex-col items-start gap-1">
                    <Badge tone={TONE[line.outcome]}>{t(`pipelines.runs.outcomes.${line.outcome}`)}</Badge>
                    {line.step != null ? (
                      <span className="text-caption text-fg-subtle">{t("pipelines.rejected.step", { step: line.step + 1 })}</span>
                    ) : null}
                  </div>
                ),
              },
              // A page of plain writes says nothing per line: the Message column waits under Columns.
              { id: "message", label: t("pipelines.runs.message"), hidden: !withMessages, text: (line) => line.message ?? "" },
            ]}
            empty={<p className="text-caption text-fg-subtle">{t(empty ? "pipelines.runs.nothingWritten" : "pipelines.runs.noLines")}</p>}
          />
          <div className="flex flex-wrap items-center gap-2">
            {cursors.length > 1 ? (
              <Button size="sm" onClick={() => setCursors([undefined])}>
                {t("pipelines.runs.newest")}
              </Button>
            ) : null}
            {page.data.next != null ? (
              <Button
                size="sm"
                onClick={() => setCursors((current) => [...current, page.data.next ?? undefined])}
              >
                {t("pipelines.runs.older")}
              </Button>
            ) : null}
          </div>
        </>
      )}
    </section>
  );
}

/** A record's entity by its own name; the whole URN on hover and for a screen reader (T-4991). */
function RecordId({ id }: { id: string }): JSX.Element {
  const short = urnName(id);
  if (!short) return <span className="font-mono text-caption [overflow-wrap:anywhere]">{id}</span>;
  return (
    // A short name keeps its letters together: the column takes its width and the table scrolls,
    // where `anywhere` broke it one letter per line at 400 px.
    <span className="whitespace-nowrap font-mono text-caption" title={id}>
      <span aria-hidden>{short}</span>
      <span className="sr-only">{id}</span>
    </span>
  );
}
