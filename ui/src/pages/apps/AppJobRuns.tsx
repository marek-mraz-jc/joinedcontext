import { useId } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import { Badge, Card } from "../../components/ui";
import type { BadgeTone } from "../../components/ui/Badge";

/** One declared job, `spec.server.jobs[]` (AP-154). */
interface Job {
  name: string;
  schedule: string;
}

/** Its last run, `status.jobs[]`, as the reconciler read it from the App's shard. */
interface JobRun {
  name: string;
  lastRun: string;
  outcome: string;
  message?: string;
  durationMs: number;
  failuresInARow: number;
}

const TONES: Record<string, BadgeTone> = {
  succeeded: "success",
  failed: "danger",
  timedOut: "danger",
  skipped: "warning",
};
const OUTCOMES = new Set(Object.keys(TONES));

/** A declared job beside its last run, one row of the grid. */
interface JobRow {
  job: Job;
  run?: JobRun;
  outcome: string | null;
}

/** The declared jobs and the last runs of an App's manifest, or nothing it does not hold. */
export function jobsOf(manifest: unknown): { jobs: Job[]; runs: JobRun[] } {
  const app = (manifest ?? {}) as { spec?: { server?: { jobs?: unknown } }; status?: { jobs?: unknown } };
  const jobs = Array.isArray(app.spec?.server?.jobs)
    ? (app.spec.server.jobs as Partial<Job>[]).filter(
        (job): job is Job => typeof job.name === "string" && typeof job.schedule === "string",
      )
    : [];
  const runs = Array.isArray(app.status?.jobs)
    ? (app.status.jobs as Partial<JobRun>[]).filter(
        (run): run is JobRun => typeof run.name === "string" && typeof run.lastRun === "string",
      )
    : [];
  return { jobs, runs };
}

/**
 * The scheduled jobs of a `wasm` App and how each last ran (AP-154, AP-155, T-3372): its
 * schedule in UTC, when it last ran, the outcome in words beside its colour, how long it took and,
 * for a run that did not succeed, the job's own sentence. An App without jobs shows nothing.
 */
export function AppJobRuns({ project, name }: { project: string; name: string }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const heading = useId();
  const app = useQuery({
    queryKey: queryKeys.resource(project, "apps", name),
    queryFn: async () =>
      unwrap<unknown>(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "apps", name } },
        }),
      ),
    retry: false,
  });
  const { jobs, runs } = jobsOf(app.data);
  if (jobs.length === 0) return null;
  const rows: JobRow[] = jobs.map((job) => {
    const run = runs.find((candidate) => candidate.name === job.name);
    return { job, run, outcome: run && OUTCOMES.has(run.outcome) ? run.outcome : null };
  });
  const time = new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "short", timeZone: "UTC" });
  const when = (iso: string) => {
    const at = new Date(iso);
    return Number.isNaN(at.getTime()) ? iso : `${time.format(at)} UTC`;
  };

  return (
    <Card as="section" padding="sm" aria-labelledby={heading} className="space-y-2">
      <h2 id={heading} className="text-sm font-semibold">
        {t("apps.jobs.title")}
      </h2>
      <ManifestGrid<JobRow>
        caption={t("apps.jobs.caption", { name })}
        storageKey="app-jobs"
        searchLabel={t("grid.searchIn", { list: t("apps.jobs.title") })}
        rows={rows}
        rowKey={(row) => row.job.name}
        query={app}
        name={{ id: "job", label: t("apps.jobs.job"), text: (row) => row.job.name, cell: (row) => <span className="font-mono text-caption">{row.job.name}</span> }}
        columns={[
          { id: "schedule", label: t("apps.jobs.schedule"), text: (row) => row.job.schedule, cell: (row) => <span className="font-mono text-caption">{row.job.schedule}</span> },
          { id: "lastRun", label: t("apps.jobs.lastRun"), text: (row) => row.run?.lastRun ?? "", cell: (row) => (row.run ? when(row.run.lastRun) : t("apps.jobs.notRun")) },
          {
            id: "outcome",
            label: t("apps.jobs.outcome"),
            filter: true,
            text: (row) => row.outcome ?? "",
            valueLabel: (value) => (OUTCOMES.has(value) ? t(`apps.jobs.outcomes.${value}`) : t("apps.jobs.notRun")),
            cell: (row) =>
              row.outcome ? (
                <span className="space-y-1">
                  <Badge tone={TONES[row.outcome]}>{t(`apps.jobs.outcomes.${row.outcome}`)}</Badge>
                  {row.run?.message ? <span className="block text-caption text-fg-muted">{row.run.message}</span> : null}
                  {row.run && row.run.failuresInARow >= 3 ? (
                    <span className="block text-caption text-danger">{t("apps.jobs.inARow", { count: row.run.failuresInARow })}</span>
                  ) : null}
                </span>
              ) : (
                <span className="text-fg-muted">{t("apps.jobs.notRun")}</span>
              ),
          },
          {
            id: "duration",
            label: t("apps.jobs.duration"),
            numeric: true,
            align: "right",
            text: (row) => (row.run && row.outcome !== "skipped" ? String(row.run.durationMs) : ""),
            cell: (row) => (row.run && row.outcome !== "skipped" ? t("apps.jobs.ms", { count: row.run.durationMs }) : "—"),
          },
          {
            id: "failuresInARow",
            label: t("apps.jobs.failuresInARow"),
            numeric: true,
            align: "right",
            hidden: true,
            text: (row) => String(row.run?.failuresInARow ?? 0),
          },
        ]}
        empty={null}
      />
      <p className="text-caption text-fg-muted">{t("apps.jobs.note")}</p>
    </Card>
  );
}
