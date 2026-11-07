import { reasonOf } from "../../api/client";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { usePermissions } from "../../api/permissions";
import { startJob } from "../../jobs";
import { Alert, Badge, Button, ExternalLink, PageFailed, PermissionGuard } from "../../components/ui";
import { standing } from "./KnowledgePage";
import { knowledgeKeys, listErrors, recrawl, timeText } from "./knowledge";
import type { KnowledgeSourceRow } from "./knowledge";

/** One count over every source; a source not crawled yet adds nothing (T-4197). */
export function sum(rows: KnowledgeSourceRow[], key: "pages" | "documents" | "passages" | "embedded" | "pending" | "failed"): number {
  return rows.reduce((total, row) => total + (row[key] ?? 0), 0);
}

/**
 * How far the assistant has indexed one source (T-4197, API/01 §34): pages, files and passages,
 * how many are searchable and how many still wait, the last run, and for a person who may steer
 * the source each failed read with why and a re-index.
 */
export function IndexStatus({ project, row }: { project: string; row: KnowledgeSourceRow }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const mayPropose = usePermissions(project).can("KnowledgeSource", "propose");
  const reindex = useMutation({
    mutationFn: () => recrawl(project, row.source),
    onSuccess: () => {
      startJob({ kind: "knowledgeCrawl", project, name: row.source });
      void queryClient.invalidateQueries({ queryKey: knowledgeKeys.sources(project) });
    },
  });
  const state = standing(row);
  const busy = row.job?.state === "queued" || row.job?.state === "running";
  const counts: [string, number][] = [
    ["knowledge.index.pages", row.pages ?? 0],
    ["knowledge.index.files", row.documents ?? 0],
    ["knowledge.index.passages", row.passages ?? 0],
    ["knowledge.index.embedded", row.embedded ?? 0],
    ["knowledge.index.pending", row.pending ?? 0],
    ["knowledge.index.unread", row.failed ?? 0],
  ];
  return (
    <section aria-labelledby="knowledge-index" className="space-y-3 rounded-md border border-border bg-surface p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="knowledge-index" className="text-lg font-semibold">
          {t("knowledge.index.title")}
        </h2>
        {row.type === "upload" ? null : (
          <PermissionGuard project={project} kind="KnowledgeSource" verb="propose">
            <Button
              variant="secondary"
              size="sm"
              disabled={busy || reindex.isPending}
              disabledReason={t(busy ? "knowledge.alreadyQueued" : "knowledge.recrawlSending")}
              onClick={() => reindex.mutate()}
            >
              {t("knowledge.index.reindex")}
            </Button>
          </PermissionGuard>
        )}
      </div>
      {reindex.isError ? (
        <Alert role="alert" tone="danger">
          {reasonOf(reindex.error, t("app.error.generic"))}
        </Alert>
      ) : null}
      {reindex.isSuccess ? (
        <Alert role="status" tone="success">
          {t("knowledge.queued", { source: row.source })}
        </Alert>
      ) : null}
      <dl className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
        {counts.map(([key, value]) => (
          <div key={key}>
            <dt className="text-caption text-fg-muted">{t(key)}</dt>
            <dd className="text-lg font-semibold tabular-nums">{value}</dd>
          </div>
        ))}
      </dl>
      <p className="flex flex-wrap items-center gap-2 text-caption text-fg-muted">
        <Badge tone={state.tone}>{t(state.key)}</Badge>
        {row.job
          ? t("knowledge.index.lastRun", {
              started: timeText(row.job.startedAt, i18n.language),
              finished: timeText(row.job.finishedAt, i18n.language),
            })
          : t("knowledge.index.noRun")}
        {row.job?.state === "failed" && row.job.error ? <span className="[overflow-wrap:anywhere]">{row.job.error}</span> : null}
      </p>
      {mayPropose && (row.failed ?? 0) > 0 ? <CrawlErrors project={project} source={row.source} /> : null}
    </section>
  );
}

/** The failed reads of a source, newest first, as the assistant recorded them. */
function CrawlErrors({ project, source }: { project: string; source: string }): JSX.Element {
  const { t } = useTranslation();
  const errors = useQuery({ queryKey: knowledgeKeys.errors(project, source), queryFn: () => listErrors(project, source) });
  if (errors.isLoading) return <p role="status">{t("app.loading")}</p>;
  if (errors.isError) return <PageFailed error={errors.error} onRetry={() => void errors.refetch()} />;
  const rows = errors.data ?? [];
  if (rows.length === 0) return <p>{t("knowledge.index.noneUnread")}</p>;
  return (
    <ul aria-label={t("knowledge.index.unreadList")} className="space-y-1">
      {rows.map((error) => (
        <li key={`${error.kind}:${error.url}`} className="flex flex-wrap items-center gap-2">
          <Badge tone="danger">{error.reason}</Badge>
          <Badge>{t(`knowledge.link.${error.kind}`)}</Badge>
          <ExternalLink href={error.url} className="font-mono [overflow-wrap:anywhere]">
            {error.url}
          </ExternalLink>
        </li>
      ))}
    </ul>
  );
}
