import { useEffect, useRef, useState } from "react";
import type { JSX } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../api/client";
import type { components } from "../api/schema";
import { dismissJob, finishJob, markJobsSeen, progressOf, useJobs } from "../jobs";
import type { Job, JobOutcome } from "../jobs";
import { knowledgeKeys, listSources } from "../pages/knowledge/knowledge";
import { Button, Icon, Menu, MenuContent, MenuItem, MenuLabel, MenuTrigger } from "./ui";

type AppBuild = components["schemas"]["AppBuild"];

/** Where a job's page is: the App, or the knowledge source. */
function pageOf(job: Job): string {
  const project = encodeURIComponent(job.project);
  return job.kind === "appBuild"
    ? `/projects/${project}/apps/${encodeURIComponent(job.name)}`
    : `/projects/${project}/knowledge/${encodeURIComponent(job.name)}`;
}

/** How one watched job stands now, read from what the platform answers. */
interface Standing {
  outcome?: JobOutcome;
  /** RFC 3339: when the work itself started, when the platform says. */
  startedAt?: string;
  typicalSeconds?: number | null;
  queued?: boolean;
}

function buildStanding(job: Job, build: AppBuild): Standing {
  const run = build.run;
  // The run that was newest when Rebuild was pressed is not this job's.
  if (!run || (run.number ?? 0) <= (job.after ?? 0)) return { queued: true, typicalSeconds: build.typicalSeconds };
  if (run.status !== "completed") {
    return { startedAt: run.startedAt ?? undefined, typicalSeconds: build.typicalSeconds, queued: !run.startedAt };
  }
  const outcome: JobOutcome =
    run.conclusion === "success" ? "succeeded" : run.conclusion === "cancelled" || run.conclusion === "skipped" ? "cancelled" : "failed";
  return { outcome };
}

/**
 * The long-running actions this person started (T-3245): how far each is, the estimate when the
 * platform has one, a notice when one finishes, and the way back to its page. Mounted in the Shell,
 * so it watches whichever page the person has gone on to.
 */
export function JobsMenu(): JSX.Element | null {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const jobs = useJobs();
  const running = jobs.filter((job) => !job.outcome);
  const [said, setSaid] = useState("");
  const [, tick] = useState(0);

  const watched = useQueries({
    queries: running.map((job) => ({
      queryKey: ["jobs", job.id, job.startedAt],
      retry: false,
      refetchInterval: 10_000,
      refetchIntervalInBackground: true,
      queryFn: async (): Promise<Standing> => {
        if (job.kind === "appBuild") {
          const build = (await unwrap(
            await api.GET("/api/v1/projects/{project}/apps/{name}/build", {
              params: { path: { project: job.project, name: job.name } },
            }),
          )) as AppBuild;
          return buildStanding(job, build);
        }
        // One list per project and interval, however many of its sources are building: the
        // jobs of one project share the read, and the knowledge page its cache (T-6230).
        const sources = await queryClient.fetchQuery({
          queryKey: knowledgeKeys.sources(job.project),
          queryFn: () => listSources(job.project),
          staleTime: 5_000,
        });
        const row = sources.find((source) => source.source === job.name);
        const state = row?.job?.state;
        if (state === "done") return { outcome: "succeeded" };
        if (state === "failed") return { outcome: "failed" };
        return { queued: state !== "running" };
      },
    })),
  });

  // A job that finished is said once, politely, and stays in the list with how it ended.
  const announced = useRef(new Set<string>());
  useEffect(() => {
    running.forEach((job, index) => {
      const outcome = watched[index]?.data?.outcome;
      if (outcome && !announced.current.has(job.id)) {
        announced.current.add(job.id);
        finishJob(job.id, outcome);
        setSaid(t(`jobs.finished.${job.kind}.${outcome}`, { name: job.name }));
      }
    });
  }, [running, watched, t]);

  // The elapsed minutes move without a new answer from the platform.
  useEffect(() => {
    if (running.length === 0) return;
    const timer = window.setInterval(() => tick((n) => n + 1), 30_000);
    return () => window.clearInterval(timer);
  }, [running.length]);

  if (jobs.length === 0) {
    return <span role="status" aria-live="polite" className="sr-only">{said}</span>;
  }
  const unseen = jobs.filter((job) => job.outcome && !job.seen).length;
  const describe = (job: Job, index: number): string => {
    if (job.outcome) return t(`jobs.outcome.${job.outcome}`);
    const standing = watched[index]?.data;
    if (watched[index]?.isError) return t("jobs.unknown");
    if (!standing || (standing.queued && !standing.startedAt)) return t("jobs.queued");
    const { elapsedMinutes, leftMinutes } = progressOf(standing.startedAt ?? job.startedAt, standing.typicalSeconds);
    return leftMinutes === undefined
      ? t("jobs.running", { minutes: elapsedMinutes })
      : t("jobs.runningWithEstimate", { minutes: elapsedMinutes, left: leftMinutes });
  };

  return (
    <>
      <span role="status" aria-live="polite" className="sr-only">
        {said}
      </span>
      <Menu onOpenChange={(open) => (open ? markJobsSeen() : undefined)}>
        <MenuTrigger asChild>
          <Button
            variant="ghost"
            className="relative px-1.5"
            aria-label={t("jobs.label", { running: running.length, finished: unseen })}
          >
            <Icon name="refresh" className={running.length > 0 ? "size-5 motion-safe:animate-spin" : "size-5"} />
            {unseen > 0 ? (
              <span
                aria-hidden="true"
                className="absolute -right-0.5 -top-0.5 min-w-4 rounded-full bg-primary px-1 text-center text-caption font-bold text-primary-fg"
              >
                {unseen}
              </span>
            ) : null}
          </Button>
        </MenuTrigger>
        <MenuContent align="end" className="w-80 max-w-full">
          <MenuLabel>{t("jobs.title")}</MenuLabel>
          {jobs.map((job) => {
            const index = running.indexOf(job);
            return (
              <MenuItem
                key={job.id}
                className="flex-col items-start gap-0.5"
                onSelect={() => {
                  if (job.outcome) dismissJob(job.id);
                  void navigate({ href: pageOf(job) });
                }}
              >
                <span className="font-semibold [overflow-wrap:anywhere]">{t(`jobs.kind.${job.kind}`, { name: job.name })}</span>
                <span className={job.outcome === "failed" ? "text-caption text-danger" : "text-caption text-fg-muted"}>
                  {describe(job, index)}
                </span>
              </MenuItem>
            );
          })}
        </MenuContent>
      </Menu>
    </>
  );
}
