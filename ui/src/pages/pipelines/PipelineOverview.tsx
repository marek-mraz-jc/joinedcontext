import { useState } from "react";
import type { JSX, ReactNode } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap, whilePending } from "../../api/client";
import { asManifests, isChange, localized } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { proposeChecked } from "../../api/proposal";
import type { components } from "../../api/schema";
import { useOrgDomain } from "../../api/projects";
import { ChangeNotice } from "../../components/ChangeNotice";
import { RecordLink } from "../../components/RecordLink";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { ResourcePageFailed } from "../../components/ui/PageState";
import { Alert, Badge, Button, PageHeader, SourceLink } from "../../components/ui";
import { TypeLink } from "../models/ModelLinks";
import { ago } from "../../components/ago";
import { nextRuns, parseCron, scheduleWords } from "./cron";
import { endpointUrn } from "./PipelineEditor";
import { PipelineRejectedDialog } from "./PipelineRejected";
import { PipelineAccessRequests } from "./AccessRequests";
import { PipelineRunsView } from "./PipelineRuns";
import { PipelineSourceReport } from "./PipelineSourceReport";

type Run = components["schemas"]["PipelineRun"];
type Rejected = components["schemas"]["Rejected"];

export interface PipelineSpec {
  class?: string;
  period?: string;
  schedule?: string;
  enabled?: boolean;
  secretRefs?: { name?: string; key?: string; envVar?: string }[];
  source?: { dataSourceRef?: { name?: string } | string };
  targetEndpoint?: string;
  output?: { type?: string };
  expiry?: { after?: string };
  sourceDeletes?: { mode?: string };
}

/** The data source a pipeline reads, by name, when it names one. */
export function dataSourceOf(spec: PipelineSpec): string | undefined {
  const ref = spec.source?.dataSourceRef;
  const name = typeof ref === "string" ? ref : ref?.name;
  return name && name.trim() !== "" ? name : undefined;
}

/** Bento's own duration notation, `250ms` / `45s` / `5m` / `1h` (Architecture/08 §1). */
export function periodSeconds(period: string | undefined): number | undefined {
  const match = /^(\d+(?:\.\d+)?)(ms|s|m|h)$/.exec(period?.trim() ?? "");
  if (!match) {
    return undefined;
  }
  const value = Number(match[1]);
  const factor = { ms: 0.001, s: 1, m: 60, h: 3600 }[match[2] as "ms" | "s" | "m" | "h"];
  return value * factor;
}

/**
 * The 30-second rule (Architecture/08 §1): `auto` runs push-based and sub-30-second pipelines
 * in the resident runner, everything slower as a CronJob. An explicit class always wins.
 */
export function executionClass(spec: PipelineSpec): "resident" | "scheduled" {
  if (spec.class === "resident" || spec.class === "scheduled") {
    return spec.class;
  }
  if (spec.schedule) {
    return "scheduled";
  }
  const seconds = periodSeconds(spec.period);
  return seconds === undefined || seconds < 30 ? "resident" : "scheduled";
}

/**
 * Pausing is a repository change like any other, never a live switch (CC-35). `status` is the
 * API's own computation (MF-04); a write never sends it back, and the gate wants a green check
 * for exactly this manifest (PF-57, T-2264).
 */
export function proposeEnabled(project: string, pipeline: Manifest, run: boolean) {
  const body = {
    apiVersion: pipeline.apiVersion,
    kind: pipeline.kind,
    metadata: pipeline.metadata,
    spec: { ...pipeline.spec, enabled: run },
  };
  return proposeChecked(project, "pipelines", body, false);
}

/** The latest run of one pipeline, or `null` when it never ran (T-4817). */
export function useLastRun(project: string, name: string) {
  return useQuery({
    queryKey: [...queryKeys.resource(project, "pipelines", name), "runs", "latest"],
    retry: false,
    refetchInterval: 60_000,
    queryFn: async () =>
      (
        await unwrap(
          await api.GET("/api/v1/projects/{project}/pipelines/{name}/runs", {
            params: { path: { project, name }, query: { limit: 1 } },
          }),
        )
      ).items[0] ?? null,
  });
}

/**
 * The latest run of every pipeline of the project the caller reads, by name, in one request
 * whatever the row count (T-5818); a pipeline that never ran is not in the map.
 */
export function useLatestRuns(project: string) {
  return useQuery({
    queryKey: [...queryKeys.list(project, "pipelines"), "latest-runs"],
    retry: false,
    refetchInterval: 60_000,
    queryFn: async () =>
      new Map(
        (
          await unwrap(
            await api.GET("/api/v1/projects/{project}/pipeline-runs", { params: { path: { project } } }),
          )
        ).items.map((item) => [item.pipeline, item.run] as const),
      ),
  });
}

/** How a run ended: a record that failed in a step reads red, a refused one amber. */
export function runTone(run: Run): "danger" | "warning" | "success" {
  return run.failed > 0 ? "danger" : run.rejected > 0 ? "warning" : "success";
}

/**
 * When a pipeline last ran and what came of it (T-4817): the time, the records written, and the
 * rejected and failed ones as links to where they are read. `undefined` is still loading; `null`
 * never ran. Counters of a resident runner, totals since it started, follow when given.
 */
export function LastRunCell({
  project,
  name,
  run,
  now,
  failed: unreadable = false,
  counters,
}: {
  project: string;
  name: string;
  run: Run | null | undefined;
  now: number;
  /** The runs could not be read. */
  failed?: boolean;
  counters?: { received?: number | null; errors?: number | null } | null;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const time = new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" });
  const count = (n: number) => n.toLocaleString(locale);
  const to = (hash: "runs" | "rejected", children: ReactNode, className: string) => (
    <Link
      to="/projects/$project/$plural/$name"
      params={{ project, plural: "pipelines", name }}
      hash={hash}
      className={`focus-ring underline underline-offset-2 hover:no-underline ${className}`}
    >
      {children}
    </Link>
  );

  let head: ReactNode;
  if (unreadable) {
    head = <span className="text-fg-subtle">{t("pipelines.lastRun.unavailable")}</span>;
  } else if (run === undefined) {
    head = <span className="text-fg-subtle">{t("app.loading")}</span>;
  } else if (run === null) {
    head = <span className="text-fg-subtle">{t("pipelines.lastRun.never")}</span>;
  } else {
    const tone = runTone(run);
    head = (
      <>
        <span className="flex flex-wrap items-center gap-1.5">
          <time dateTime={run.lastAt} title={time.format(new Date(run.lastAt))} className="tabular-nums text-fg">
            {ago(run.lastAt, now, t)}
          </time>
          {tone === "danger" ? <Badge tone="danger">{t("pipelines.lastRun.runFailedBadge")}</Badge> : null}
        </span>
        <span className="block tabular-nums">
          <span className="text-fg">{t("pipelines.lastRun.written", { count: run.sent })}</span>
          {run.rejected > 0 ? (
            <>
              {" · "}
              {to("rejected", t("pipelines.lastRun.rejected", { n: count(run.rejected) }), "text-warning")}
            </>
          ) : null}
          {run.failed > 0 ? (
            <>
              {" · "}
              {to("runs", t("pipelines.lastRun.failedCount", { n: count(run.failed) }), "font-medium text-danger")}
            </>
          ) : null}
        </span>
      </>
    );
  }

  return (
    <div className="flex flex-col gap-0.5 text-caption" data-testid="pipeline-last-run">
      {head}
      {counters && (counters.received != null || counters.errors != null) ? (
        <span className="block tabular-nums text-fg-subtle">
          {t("pipelines.lastRun.sinceStart")}{" "}
          {counters.received != null ? t("pipelines.lastRun.received", { n: count(counters.received) }) : null}
          {counters.received != null && counters.errors ? " · " : null}
          {counters.errors
            ? to("runs", t("pipelines.lastRun.errorCount", { n: count(counters.errors) }), "font-medium text-danger")
            : null}
        </span>
      ) : null}
    </div>
  );
}

/** When the pipeline runs next: a schedule's next tick, the period after the last run, or why none. */
export function nextRunOf(
  spec: PipelineSpec,
  running: boolean,
  last: Run | null | undefined,
  now: number,
): { kind: "paused" | "continuous" | "unknown" } | { kind: "at"; at: Date } {
  if (!running) {
    return { kind: "paused" };
  }
  if (executionClass(spec) === "resident") {
    return { kind: "continuous" };
  }
  const cron = spec.schedule ? parseCron(spec.schedule) : undefined;
  if (cron) {
    const [at] = nextRuns(cron, new Date(now), 1);
    return at ? { kind: "at", at } : { kind: "unknown" };
  }
  const seconds = periodSeconds(spec.period);
  if (seconds !== undefined && last) {
    const at = Date.parse(last.lastAt) + seconds * 1000;
    return { kind: "at", at: new Date(Math.max(at, now)) };
  }
  return { kind: "unknown" };
}

function Fact({ label, children }: { label: string; children: ReactNode }): JSX.Element {
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <dt className="text-caption text-fg-subtle">{label}</dt>
      <dd className="text-body text-fg [overflow-wrap:anywhere]">{children}</dd>
    </div>
  );
}

const REJECTED_SHOWN = 5;

/**
 * One pipeline, read-only (T-4817): where it reads and writes, how it runs, when it last ran and
 * with what result, when it runs next, its runs with their log, and the newest records the model
 * refused. Pause and Edit are there for a role that may propose; a reader sees the rest.
 */
export function PipelineOverview({ project, name }: { project: string; name: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const time = new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" });
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const orgDomain = useOrgDomain(project);
  const [now] = useState(() => Date.now());
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [rejectedOpen, setRejectedOpen] = useState(false);
  const [openRun, setOpenRun] = useState<string | undefined>(undefined);

  const pipeline = useQuery({
    queryKey: queryKeys.resource(project, "pipelines", name),
    retry: false,
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "pipelines", name } },
        }),
      ),
  });
  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });
  const last = useLastRun(project, name);
  const counterSpec = pipeline.data?.spec as PipelineSpec | undefined;
  const counters = useStreamCounters(
    project,
    name,
    counterSpec !== undefined && counterSpec.enabled !== false && executionClass(counterSpec) === "resident",
  );
  const rejected = useQuery({
    queryKey: [...queryKeys.resource(project, "pipelines", name), "rejected", "newest", REJECTED_SHOWN],
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/rejected", {
          params: { path: { project, name }, query: { limit: REJECTED_SHOWN } },
        }),
      ),
  });

  const toggle = useMutation({
    mutationFn: async (run: boolean) => {
      setError(null);
      return proposeEnabled(project, pipeline.data as Manifest, run);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.resource(project, "pipelines", name) });
    },
    onError: (err) => {
      setError(err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic"));
    },
  });

  const back = (
    <Link
      to="/projects/$project/$plural"
      params={{ project, plural: "pipelines" }}
      className="focus-ring text-sm text-primary-soft-fg underline hover:no-underline"
    >
      {t("pipelines.overview.back")}
    </Link>
  );

  if (pipeline.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }
  if (pipeline.isError) {
    return (
      <ResourcePageFailed
        title={name}
        description={t("pipelines.lead")}
        error={pipeline.error}
        onRetry={() => void pipeline.refetch()}
        back={back}
      />
    );
  }

  const manifest = pipeline.data as Manifest;
  const spec = manifest.spec as PipelineSpec;
  const klass = executionClass(spec);
  // Absent means running: a pipeline is only paused by an explicit `false`.
  const running = spec.enabled !== false;
  const phase = (manifest.status?.phase ?? "").toLowerCase();
  const source = dataSourceOf(spec);
  const target = asManifests(endpoints.data?.items ?? []).find(
    (endpoint) => spec.targetEndpoint !== undefined && endpointUrn(orgDomain, endpoint) === spec.targetEndpoint,
  );
  const space = (target?.spec as { contextSpaceRef?: unknown } | undefined)?.contextSpaceRef;
  const next = nextRunOf(spec, running, last.data, now);
  const runsFromQueue = manifest.status?.conditions?.some((condition) => condition.type === "Scheduled");
  const stalled = (manifest.status?.conditions ?? []).find(
    (condition) => condition.type === "StreamWriting" && condition.status === "False",
  );
  const refusedRecords: Rejected[] = rejected.data?.items ?? [];
  const scheduleText = spec.schedule ? scheduleWords(spec.schedule, t, locale) : undefined;

  return (
    <div className="flex flex-col gap-section">
      <div className="flex flex-col gap-2">
        {/* One header row: the title with its id, the actions on the same row; the breadcrumb leads back. */}
        <PageHeader
          title={localized(manifest.metadata.title, locale, name)}
          // The id beside the title; on a phone the breadcrumb already ends with it.
          titleAction={<code className="ms-2 hidden font-mono text-caption text-fg-muted sm:inline">{name}</code>}
          actions={
            <div className="flex flex-wrap items-center gap-2">
              <PermissionGuard project={project} kind="Pipeline" verb="propose">
                <Button
                  disabled={toggle.isPending}
                  onClick={() => toggle.mutate(!running)}
                  title={t(running ? "pipelines.pauseHint" : "pipelines.resumeHint")}
                >
                  {t(running ? "pipelines.pause" : "pipelines.resume")}
                </Button>
              </PermissionGuard>
              <PermissionGuard project={project} kind="Pipeline" verb="propose">
                <Button
                  variant="primary"
                  onClick={() =>
                    void navigate({
                      to: "/projects/$project/$plural/$name/edit",
                      params: { project, plural: "pipelines", name },
                    })
                  }
                >
                  {t("pipelines.edit")}
                </Button>
              </PermissionGuard>
              {manifest.status?.sourceUrl ? (
                <SourceLink href={manifest.status.sourceUrl} label={t("spaces.field.source")} />
              ) : null}
            </div>
          }
        />
      </div>

      {change ? <ChangeNotice change={change} project={project} /> : null}
      {error ? (
        <Alert role="alert" tone="danger">
          {error}
        </Alert>
      ) : null}
      {stalled ? (
        <Alert role="status" tone="warning">
          {t("pipelines.notWriting")}
          {stalled.message || stalled.reason ? `: ${stalled.message ?? stalled.reason}` : null}
        </Alert>
      ) : null}

      <section aria-labelledby="pipeline-facts" className="flex flex-col gap-3">
        <h2 id="pipeline-facts" className="text-title text-fg">
          {t("pipelines.overview.facts")}
        </h2>
        <dl className="grid grid-cols-2 gap-x-4 gap-y-3 lg:grid-cols-3">
          <Fact label={t("pipelines.field.phase")}>
            <span className="flex flex-wrap items-center gap-1.5">
              <LifecycleBadge
                kind="phase"
                value={!running && ["live", "applied", "synced"].includes(phase) ? "paused" : manifest.status?.phase}
              />
              <Badge tone={klass === "resident" ? "info" : "neutral"} title={t(`pipelines.class.${klass}Help`)}>
                {t(`pipelines.class.${klass}`)}
              </Badge>
            </span>
          </Fact>
          <Fact label={t("pipelines.field.dataSource")}>
            {source ? (
              <RecordLink project={project} plural="datasources" name={source} className="font-mono" />
            ) : (
              <span className="text-fg-subtle">{t("pipelines.overview.noDataSource")}</span>
            )}
          </Fact>
          <Fact label={t("pipelines.field.targetEndpoint")}>
            {target ? (
              <RecordLink project={project} plural="endpoints" name={target.metadata.name} className="font-mono" />
            ) : spec.targetEndpoint ? (
              <span className="font-mono text-caption">{spec.targetEndpoint}</span>
            ) : (
              <span className="text-fg-subtle">{t("pipelines.overview.noTarget")}</span>
            )}
            {typeof space === "string" && space !== "" ? (
              <span className="block text-caption text-fg-subtle">
                {t("pipelines.overview.inSpace")}{" "}
                <RecordLink project={project} plural="spaces" name={space} className="font-mono" />
              </span>
            ) : null}
          </Fact>
          <Fact label={t("pipelines.overview.writes")}>
            {typeof spec.output?.type === "string" && spec.output.type !== "" ? (
              <TypeLink project={project} type={spec.output.type} className="font-mono" />
            ) : (
              <span className="text-fg-subtle">{t("pipelines.overview.writesUnknown")}</span>
            )}
          </Fact>
          <Fact label={t("pipelines.field.schedule")}>
            {spec.schedule ? (
              // In words when the cron says one of the form's choices; the cron itself on hover.
              <span title={spec.schedule} className={scheduleText ? undefined : "font-mono"}>
                {scheduleText ?? spec.schedule}
              </span>
            ) : spec.period ? (
              t("pipelines.overview.every", { period: spec.period })
            ) : (
              t("pipelines.overview.pushBased")
            )}
          </Fact>
          {(spec.secretRefs ?? []).length > 0 ? (
            // Names only: a secret's value is resolved by the reconciler and never leaves the
            // cluster (PL-17).
            <Fact label={t("pipelines.overview.credentials")}>
              <ul className="flex flex-wrap gap-1">
                {(spec.secretRefs ?? []).map((ref) => (
                  <li key={`${ref.name}/${ref.key}`}>
                    <Badge mono>
                      {ref.name}/{ref.key}
                    </Badge>
                  </li>
                ))}
              </ul>
            </Fact>
          ) : null}
          <Fact label={t("pipelines.overview.lastRun")}>
            <LastRunCell
              project={project}
              name={name}
              run={last.data}
              failed={last.isError}
              now={Math.max(now, last.dataUpdatedAt)}
              counters={counters.data}
            />
          </Fact>
          <Fact label={t("pipelines.overview.nextRun")}>
            {next.kind === "at" ? (
              <time dateTime={next.at.toISOString()} className="tabular-nums">
                {time.format(next.at)}
              </time>
            ) : (
              <span className="text-fg-subtle">{t(`pipelines.overview.next.${next.kind}`)}</span>
            )}
          </Fact>
        </dl>
      </section>

      <section id="runs" aria-labelledby="pipeline-runs" className="flex scroll-mt-4 flex-col gap-3">
        <h2 id="pipeline-runs" className="text-title text-fg">
          {t("pipelines.overview.runs")}
        </h2>
        <PipelineRunsView
          key={openRun ?? "newest"}
          project={project}
          name={name}
          initialRun={openRun}
          runsFromQueue={runsFromQueue}
        />
      </section>

      <section id="source" aria-labelledby="pipeline-source" className="flex scroll-mt-4 flex-col gap-3">
        <h2 id="pipeline-source" className="text-title text-fg">
          {t("pipelines.source.section")}
        </h2>
        <PipelineSourceReport project={project} name={name} snapshot={spec.sourceDeletes?.mode === "snapshot"} />
      </section>

      <section id="access" aria-labelledby="pipeline-access" className="flex scroll-mt-4 flex-col gap-3">
        <h2 id="pipeline-access" className="text-title text-fg">
          {t("pipelines.access.section")}
        </h2>
        <PipelineAccessRequests project={project} name={name} />
      </section>

      <section id="rejected" aria-labelledby="pipeline-rejected" className="flex scroll-mt-4 flex-col gap-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h2 id="pipeline-rejected" className="text-title text-fg">
            {t("pipelines.overview.rejected")}
          </h2>
          {refusedRecords.length > 0 ? (
            <Button onClick={() => setRejectedOpen(true)}>{t("pipelines.rejected.open")}</Button>
          ) : null}
        </div>
        {rejected.isError ? (
          <p className="text-sm text-fg-subtle">{t("pipelines.overview.rejectedUnavailable")}</p>
        ) : rejected.isPending ? (
          <p role="status" className="text-sm text-fg-subtle">
            {t("app.loading")}
          </p>
        ) : refusedRecords.length === 0 ? (
          <p className="text-sm text-fg-subtle">{t("pipelines.overview.noRejected")}</p>
        ) : (
          <ul className="flex flex-col divide-y divide-border rounded-lg border border-border">
            {refusedRecords.map((record) => (
              <li key={record.id} className="flex flex-col gap-0.5 px-4 py-3">
                <span className="flex flex-wrap items-center gap-2 text-caption text-fg-subtle">
                  <time dateTime={record.at} className="tabular-nums">
                    {time.format(new Date(record.at))}
                  </time>
                  <Badge tone="warning" mono>
                    {record.rule}
                  </Badge>
                  {record.path ? <span className="font-mono">{record.path}</span> : null}
                </span>
                <span className="text-body text-fg [overflow-wrap:anywhere]">{record.message}</span>
              </li>
            ))}
          </ul>
        )}
      </section>

      {rejectedOpen ? (
        <PipelineRejectedDialog
          project={project}
          name={name}
          onClose={() => setRejectedOpen(false)}
          onOpenRun={(run) => {
            setRejectedOpen(false);
            setOpenRun(run);
            document.getElementById("runs")?.scrollIntoView?.();
          }}
        />
      ) : null}
    </div>
  );
}

/**
 * A resident runner's totals since it started, polled while it runs; `null` from a runner that
 * does not answer, so the row stays readable rather than red (the manifest is still fine).
 */
export function useStreamCounters(project: string, name: string, enabled: boolean) {
  return useQuery({
    queryKey: [...queryKeys.resource(project, "pipelines", name), "metrics"],
    enabled,
    refetchInterval: 15_000,
    retry: false,
    queryFn: async () => {
      const answer = await api.GET("/api/v1/projects/{project}/pipelines/{name}/metrics", {
        params: { path: { project, name } },
      });
      return answer.data ?? null;
    },
  });
}
