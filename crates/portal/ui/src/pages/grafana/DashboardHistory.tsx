/**
 * A builder dashboard's versions (T-7150): each approved Change of its file, newest first, with
 * who wrote it and when. A version compares with the newest as the choices that differ, the diff
 * every Change shows; a person who may build dashboards restores one, asked twice, as a Change
 * that waits for approval like every edit. The server checks every right again.
 */
import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { reasonOf } from "../../api/client";
import { door, doorAction, type RestAnswer } from "../../api/odata";
import { Alert, Button, buttonClass, Card, EmptyState, Icon, Skeleton } from "../../components/ui";
import { PlanDiffViewer } from "../../components/diff/PlanDiffViewer";
import type { FieldChange } from "../../components/diff/PlanDiffViewer";

type Version = RestAnswer<"builder_versions">["items"][number];

/** Every leaf of `value` by its dotted path (`panels[1].title`), arrays by index. */
function leaves(value: unknown, at: string, out: Map<string, unknown>): Map<string, unknown> {
  if (Array.isArray(value)) {
    value.forEach((item, i) => leaves(item, `${at}[${i}]`, out));
  } else if (value !== null && typeof value === "object") {
    for (const [key, item] of Object.entries(value)) leaves(item, at ? `${at}.${key}` : key, out);
  } else if (at) {
    out.set(at, value);
  }
  return out;
}

/** The choices that differ between two versions of a dashboard, as the Change diff shows them. */
export function boardChanges(before: unknown, after: unknown): FieldChange[] {
  const from = leaves(before, "", new Map());
  const to = leaves(after, "", new Map());
  const paths = [...new Set([...from.keys(), ...to.keys()])].sort();
  return paths
    .filter((path) => JSON.stringify(from.get(path)) !== JSON.stringify(to.get(path)))
    .map((path) => ({
      path,
      ...(from.has(path) ? { from: from.get(path) as Record<string, never> } : {}),
      ...(to.has(path) ? { to: to.get(path) as Record<string, never> } : {}),
    }));
}

function useVersionBoard(project: string, uid: string, sha: string | undefined) {
  return useQuery({
    queryKey: ["projects", project, "grafana", "builder", uid, "versions", sha],
    enabled: sha !== undefined,
    retry: false,
    queryFn: async () =>
      (await door("portal_grafana_builder_version", { uid, sha: sha ?? "" }, project)) as RestAnswer<"builder_version">,
  });
}

export function DashboardHistory({ project, uid, mayEdit }: { project: string; uid: string; mayEdit: boolean }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const versions = useQuery({
    queryKey: ["projects", project, "grafana", "builder", uid, "versions"],
    retry: false,
    queryFn: async () => (await door("portal_grafana_builder_versions", { uid }, project)) as RestAnswer<"builder_versions">,
  });
  const [compared, setCompared] = useState<string | undefined>();
  const [asked, setAsked] = useState<Version | undefined>();
  const items = versions.data?.items ?? [];
  const newest = items[0]?.sha;
  const older = useVersionBoard(project, uid, compared);
  const current = useVersionBoard(project, uid, compared ? newest : undefined);
  const restore = useMutation({
    mutationFn: async (sha: string) =>
      (await doorAction("portal_grafana_builder_restore", { uid, sha }, project)) as RestAnswer<"builder_restore">,
    onSuccess: async () => {
      setAsked(undefined);
      setCompared(undefined);
      await queryClient.invalidateQueries({ queryKey: ["projects", project, "grafana", "builder"] });
    },
  });
  const when = (date: string) => {
    const parsed = new Date(date);
    return Number.isNaN(parsed.getTime())
      ? date
      : new Intl.DateTimeFormat(i18n.resolvedLanguage ?? "en", { dateStyle: "medium", timeStyle: "short" }).format(parsed);
  };
  const shortSha = (sha: string) => sha.slice(0, 7);
  return (
    <Card as="section" padding="md" aria-labelledby="dashboard-history-heading" className="flex flex-col gap-3">
      <div>
        <h2 id="dashboard-history-heading" className="text-title font-semibold text-fg">
          {t("dashboardHistory.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("dashboardHistory.lead")}</p>
        {mayEdit ? null : (
          <p id="dashboard-history-reader" className="mt-1 text-body text-fg">
            <Icon name="lock" className="mr-1 inline size-4 align-text-bottom" />
            {t("dashboardHistory.readerHint")}
          </p>
        )}
      </div>
      {versions.isPending ? <Skeleton className="h-24" /> : null}
      {versions.isError ? <Alert tone="danger" title={t("dashboardHistory.failed")}>{reasonOf(versions.error)}</Alert> : null}
      {versions.isSuccess && items.length === 0 ? (
        <EmptyState
          bare
          icon="dashboards"
          title={t("dashboardHistory.emptyTitle")}
          description={t("dashboardHistory.empty")}
          action={
            <Link to="/projects/$project/approvals" params={{ project }} className={buttonClass("secondary", "sm")}>
              {t("dashboardHistory.openApprovals")}
            </Link>
          }
        />
      ) : null}
      {restore.isError ? <Alert tone="danger" title={t("dashboardHistory.restoreFailed")}>{reasonOf(restore.error)}</Alert> : null}
      {items.length > 0 ? (
        <ol className="flex flex-col divide-y divide-border" aria-label={t("dashboardHistory.title")}>
          {items.map((version, index) => (
            <li key={version.sha} className={`flex flex-wrap items-center gap-x-4 gap-y-2 py-2.5${compared === version.sha ? " -mx-2 rounded-md bg-surface-subtle px-2" : ""}`}>
              <div className="min-w-0 flex-1 max-sm:basis-full">
                <p className="break-words text-body font-medium text-fg">
                  {version.message || t("dashboardHistory.noMessage")}
                  {index === 0 ? <span className="ml-2 text-caption font-normal text-fg-muted">{t("dashboardHistory.newest")}</span> : null}
                </p>
                <p className="text-caption text-fg-muted">
                  {t("dashboardHistory.byOn", { author: version.author || t("dashboardHistory.unknownAuthor"), date: when(version.date) })}{" "}
                  <span aria-hidden="true">·</span> <span className="font-mono">{shortSha(version.sha)}</span>
                </p>
              </div>
              {index > 0 ? (
                <div className="flex flex-wrap gap-2">
                  <Button
                    size="sm"
                    variant={compared === version.sha ? "primary" : "secondary"}
                    aria-pressed={compared === version.sha}
                    className="touch-target"
                    onClick={() => setCompared(compared === version.sha ? undefined : version.sha)}
                  >
                    <Icon name="git" className="size-4" />
                    {t("dashboardHistory.compare")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="touch-target"
                    disabled={!mayEdit || restore.isPending || (restore.isSuccess && restore.variables === version.sha)}
                    title={t(mayEdit ? "dashboardHistory.restoreHint" : "dashboardHistory.readerHint")}
                    aria-describedby={mayEdit ? undefined : "dashboard-history-reader"}
                    onClick={() => {
                      restore.reset();
                      setAsked(version);
                    }}
                  >
                    <Icon name="refresh" className="size-4" />
                    {t("dashboardHistory.restore")}
                  </Button>
                </div>
              ) : null}
              {restore.isSuccess && restore.variables === version.sha ? (
                <Alert className="basis-full" tone="success" title={t("dashboardHistory.restoredTitle")}>
                  {t("dashboardHistory.restored")}{" "}
                  <Link to="/projects/$project/approvals/$id" params={{ project, id: restore.data.change.metadata.name }} className="underline">
                    {t("dashboardHistory.openChange")}
                  </Link>
                </Alert>
              ) : null}
              {asked?.sha === version.sha ? (
                <Alert
                  className="basis-full"
                  tone="info"
                  title={t("dashboardHistory.confirmTitle")}
                  actions={
                    <>
                      <Button size="sm" variant="primary" disabled={restore.isPending} title={t("dashboardHistory.restoreHint")} onClick={() => restore.mutate(asked.sha)}>
                        {t("dashboardHistory.confirm")}
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => setAsked(undefined)}>
                        {t("dashboardHistory.cancel")}
                      </Button>
                    </>
                  }
                >
                  {t("dashboardHistory.confirmBody", { author: asked.author || t("dashboardHistory.unknownAuthor"), date: when(asked.date) })}
                </Alert>
              ) : null}
              {compared === version.sha ? (
                <div className="mt-2 flex basis-full flex-col gap-2 border-l-2 border-primary pl-3">
                  <h3 className="text-body font-semibold text-fg">
                    {t("dashboardHistory.diffTitle", { from: shortSha(compared), to: newest ? shortSha(newest) : "" })}
                  </h3>
                  {older.isPending || current.isPending ? <Skeleton className="h-24" /> : null}
                  {older.isError || current.isError ? (
                    <Alert tone="warning" title={t("dashboardHistory.cannotCompare")}>
                      {reasonOf(older.error ?? current.error)}
                    </Alert>
                  ) : null}
                  {older.isSuccess && current.isSuccess ? (
                    <div className="overflow-x-auto">
                      <PlanDiffViewer fields={boardChanges(older.data.board, current.data.board)} />
                    </div>
                  ) : null}
                </div>
              ) : null}
            </li>
          ))}
        </ol>
      ) : null}
    </Card>
  );
}
