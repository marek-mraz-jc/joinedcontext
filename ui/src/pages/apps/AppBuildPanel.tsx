import type { JSX } from "react";
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { startJob } from "../../jobs";
import type { components } from "../../api/schema";
import { Alert, Badge, Button, Card, ExternalLink } from "../../components/ui";

type AppBuild = components["schemas"]["AppBuild"];
type WorkflowRun = components["schemas"]["WorkflowRun"];
type AppBuilds = components["schemas"]["AppBuilds"];
type Restored = components["schemas"]["Restored"];

/** The forge's run state in the words the catalog uses (AP-86). */
export function runState(run: WorkflowRun): "building" | "succeeded" | "failed" | "cancelled" {
  if (run.status !== "completed") return "building";
  if (run.conclusion === "success") return "succeeded";
  if (run.conclusion === "cancelled" || run.conclusion === "skipped") return "cancelled";
  return "failed";
}

const buildKey = (project: string, name: string) => ["projects", project, "apps", name, "build"];

/** Where the App is built, one query the catalog card and the App page share. */
export function useAppBuild(project: string, name: string) {
  return useQuery({
    queryKey: buildKey(project, name),
    queryFn: async (): Promise<AppBuild> =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/apps/{name}/build", {
          params: { path: { project, name } },
        }),
      ),
    retry: false,
  });
}

/** Rebuild: a `workflow_dispatch` of the reviewed `build.yml` on the default branch (AP-103). */
export function useRebuild(project: string, name: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/apps/{name}/rebuild", {
          params: { path: { project, name } },
        }),
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: buildKey(project, name) });
    },
  });
}

/**
 * The build state on a published App's catalog card (AP-86): building while its newest run is in
 * progress, build failed with the run's log when that run failed and nothing newer is served,
 * otherwise served with the commit `status.build` names. An App with no repository of its own
 * that nothing ships says so (AP-87), instead of a card that silently has no Open.
 */
export function AppBuildState({
  project,
  name,
  served,
  shipped,
}: {
  project: string;
  name: string;
  served: string | null;
  shipped: boolean;
}): JSX.Element | null {
  const { t } = useTranslation();
  const build = useAppBuild(project, name);
  if (!build.data) return null;
  if (!build.data.repositoryUrl) {
    return shipped ? null : <p className="text-xs text-fg-muted">{t("apps.build.state.noRepository")}</p>;
  }
  const run = build.data.run ?? null;
  const state = run ? runState(run) : null;
  // The commit is for whoever looks for it, in the tooltip; the card says what state the app is
  // in (T-2759, AP-86): "Served dc96941" read as a code, not as "it runs".
  if (run && state === "building") {
    return (
      <Badge tone="info" title={t("apps.build.state.commit", { commit: run.commit.slice(0, 7) })}>
        {t("apps.build.state.building")}
      </Badge>
    );
  }
  // A failed build no longer flags the catalog card (owner, 2026-10-13): the card says what is
  // served; the App's own page still links the failed run.
  return served ? (
    <Badge tone="success" title={t("apps.build.state.commit", { commit: served.slice(0, 7) })}>
      {t("apps.build.state.served")}
    </Badge>
  ) : null;
}

/**
 * Where an application built on the forge is built (AP-103, ADR-N-028): its repository, the
 * newest workflow run and the package of its build, each opening in the forge behind the forge's
 * sign-in (PF-81), and Rebuild, which dispatches the reviewed `build.yml` again. Rebuild stays
 * focusable when it is not offered, and says why (UI-44).
 *
 * Nothing is drawn for an App this person may not read, or one that is not published yet.
 */
export function AppBuildPanel({ project, name }: { project: string; name: string }): JSX.Element | null {
  const { t } = useTranslation();
  const [started, setStarted] = useState(false);
  const build = useAppBuild(project, name);
  const rebuild = useRebuild(project, name);

  if (build.isPending) return null;
  if (build.isError) {
    if (build.error instanceof ApiError && build.error.status === 404) return null;
    return <Alert tone="warning">{t("apps.build.unreadable")}</Alert>;
  }
  const data = build.data;
  if (!data.repositoryUrl) {
    return <p className="text-sm text-fg-muted">{t("apps.build.notOnForge")}</p>;
  }
  const run = data.run ?? null;
  const rebuildError = rebuild.error
    ? rebuild.error instanceof ApiError
      ? (rebuild.error.problem?.detail ?? rebuild.error.message)
      : t("app.error.generic")
    : null;

  return (
    <Card as="section" padding="md" aria-labelledby="app-build-heading" className="space-y-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="app-build-heading" className="text-sm font-semibold">
          {t("apps.build.title")}
        </h2>
        <Button
          size="sm"
          loading={rebuild.isPending}
          disabled={!data.rebuild.allowed}
          disabledReason={data.rebuild.reason ?? undefined}
          onClick={() => {
            setStarted(false);
            rebuild.mutate(undefined, {
              onSuccess: () => {
                setStarted(true);
                // The person may leave: the Shell says when this build is done (T-3245).
                startJob({ kind: "appBuild", project, name, after: data.run?.number ?? 0 });
              },
            });
          }}
        >
          {t("apps.build.rebuild")}
        </Button>
      </div>
      <p className="text-sm">
        {run
          ? t(`apps.build.run.${runState(run)}`, { commit: run.commit.slice(0, 7) })
          : t("apps.build.run.none")}
      </p>
      <ul className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
        <li>
          <ExternalLink href={data.repositoryUrl}>{t("apps.build.repository")}</ExternalLink>
        </li>
        {run ? (
          <li>
            <ExternalLink href={run.url}>{t("apps.build.runLink")}</ExternalLink>
          </li>
        ) : null}
        {data.packageUrl ? (
          <li>
            <ExternalLink href={data.packageUrl}>{t("apps.build.package")}</ExternalLink>
          </li>
        ) : null}
      </ul>
      <div aria-live="polite">
        {started ? <p className="text-sm">{t("apps.build.started")}</p> : null}
      </div>
      {rebuildError ? <Alert tone="danger">{rebuildError}</Alert> : null}
      <AppBuildHistory project={project} name={name} />
    </Card>
  );
}

const problemText = (error: unknown, generic: string): string =>
  error instanceof ApiError ? (error.problem?.detail ?? error.message) : generic;

/**
 * The App's earlier successful builds, newest first, with Restore beside each one it does not
 * serve now (AP-171): Restore opens a merge request that brings the repository back to that
 * build's commit, and merging it builds that source. Nothing is drawn while the list is not
 * readable; the data note says what Restore does not roll back.
 */
function AppBuildHistory({ project, name }: { project: string; name: string }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const builds = useQuery({
    queryKey: [...buildKey(project, name), "history"],
    queryFn: async (): Promise<AppBuilds> =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/apps/{name}/builds", {
          params: { path: { project, name } },
        }),
      ),
    retry: false,
  });
  const restore = useMutation({
    mutationFn: async (commit: string): Promise<Restored> =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/apps/{name}/restore", {
          params: { path: { project, name } },
          body: { commit },
        }),
      ),
  });
  const earlier = builds.data?.builds.filter((build) => !build.current) ?? [];
  if (!builds.data || earlier.length === 0) return null;
  const { restore: offer } = builds.data;
  return (
    <div className="space-y-2 border-t border-border pt-2">
      <h3 className="text-sm font-semibold">{t("apps.build.history.title")}</h3>
      <ul className="space-y-1 text-sm">
        {earlier.map((build) => (
          <li key={build.commit} className="flex flex-wrap items-center justify-between gap-2">
            <span>
              {t("apps.build.history.entry", {
                commit: build.commit.slice(0, 7),
                at: build.completedAt ? new Date(build.completedAt).toLocaleString(i18n.language, { dateStyle: "medium", timeStyle: "short" }) : "",
              })}
            </span>
            <Button
              size="sm"
              variant="secondary"
              loading={restore.isPending && restore.variables === build.commit}
              disabled={!offer.allowed || restore.isPending}
              disabledReason={offer.reason ?? undefined}
              aria-label={t("apps.build.history.restoreOne", { commit: build.commit.slice(0, 7) })}
              onClick={() => restore.mutate(build.commit)}
            >
              {t("apps.build.history.restore")}
            </Button>
          </li>
        ))}
      </ul>
      <p className="text-xs text-fg-muted">{t("apps.build.history.dataNote")}</p>
      <div aria-live="polite">
        {restore.data ? (
          <p className="text-sm">
            {t("apps.build.history.opened")}{" "}
            <ExternalLink href={restore.data.pullRequestUrl}>{t("apps.build.history.request")}</ExternalLink>
          </p>
        ) : null}
      </div>
      {restore.error ? <Alert tone="danger">{problemText(restore.error, t("app.error.generic"))}</Alert> : null}
    </div>
  );
}
