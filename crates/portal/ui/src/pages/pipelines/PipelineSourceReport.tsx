import { useState } from "react";
import type { JSX, ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import {
  Alert,
  Badge,
  Button,
  EmptyState,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { csvField } from "../explore/exportView";
import { download } from "../models/diagramExport";

type Report = components["schemas"]["PipelineSourceDeletes"];

/** One CSV cell: quoted where needed, and an id a spreadsheet would run as a formula kept as text (F-UI3-009). */
const cell = (text: string): string => csvField(text, ",", false);

/**
 * The report as a CSV a steward keeps or hands on (T-4892): one row per id, its side, and for a
 * removal the run, the time and the reason. Ids only, never a value.
 */
export function sourceCsv(report: Report): string {
  const rows: string[][] = [["side", "id", "run", "at", "reason"]];
  report.orphanIds.forEach((id) => rows.push(["orphan", id, "", "", ""]));
  report.missingIds.forEach((id) => rows.push(["missing", id, report.lastComplete?.run ?? "", "", ""]));
  report.removed.forEach((r) => rows.push(["removed", r.id, r.run, r.at, r.reason]));
  return rows.map((row) => row.map(cell).join(",")).join("\r\n") + "\r\n";
}

function Count({ label, hint, children }: { label: string; hint: string; children: ReactNode }): JSX.Element {
  return (
    <div className="flex flex-col gap-0.5 rounded-lg border border-border px-4 py-3">
      <dt className="text-caption text-fg-subtle">{label}</dt>
      <dd className="text-title tabular-nums text-fg">{children}</dd>
      <dd className="text-caption text-fg-muted">{hint}</dd>
    </div>
  );
}

function Ids({ title, ids, total, testId }: { title: string; ids: string[]; total: number; testId: string }): JSX.Element | null {
  const { t } = useTranslation();
  if (total === 0) return null;
  return (
    <details className="rounded-lg border border-border px-4 py-3" data-testid={testId}>
      <summary className="cursor-pointer text-body font-semibold text-fg">{title}</summary>
      <ul className="mt-2 flex flex-col gap-1">
        {ids.map((id) => (
          <li key={id} className="font-mono text-caption text-fg [overflow-wrap:anywhere]">
            {id}
          </li>
        ))}
      </ul>
      {total > ids.length ? (
        <p className="mt-2 text-caption text-fg-muted">{t("pipelines.source.more", { count: total - ids.length })}</p>
      ) : null}
    </details>
  );
}

/**
 * "Source vs space" (T-4892, PL-97, PL-100): what the pipeline's last complete run read beside
 * what the pipeline keeps in its space, the orphans (in the space, gone at the source), the
 * records missing from the space (read, refused by the model), what it deleted and why, and a
 * held run with its decision. Shown for a snapshot pipeline; any other says how to become one.
 */
export function PipelineSourceReport({
  project,
  name,
  snapshot,
}: {
  project: string;
  name: string;
  /** `spec.sourceDeletes.mode` is `snapshot`: the only mode with a ledger to report. */
  snapshot: boolean;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [refused, setRefused] = useState<string | null>(null);
  const key = [...queryKeys.resource(project, "pipelines", name), "source-deletes"];
  const report = useQuery({
    queryKey: key,
    enabled: snapshot,
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/source-deletes", {
          params: { path: { project, name } },
        }),
      ),
  });
  const decide = useMutation({
    mutationFn: async (decision: "delete" | "keep") => {
      setRefused(null);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/pipelines/{name}/source-deletes", {
          params: { path: { project, name } },
          body: { decision },
        }),
      );
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: key }),
    onError: (err) => setRefused(err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic")),
  });

  if (!snapshot) {
    return <EmptyState bare icon="inbox" title={t("pipelines.source.notSnapshot")} description={t("pipelines.source.notSnapshotHint")} />;
  }
  if (report.isPending) {
    return (
      <p role="status" className="text-sm text-fg-subtle">
        {t("app.loading")}
      </p>
    );
  }
  if (report.isError) {
    return (
      <Alert tone="danger">
        {report.error instanceof ApiError ? (report.error.problem?.detail ?? report.error.message) : t("pipelines.source.unavailable")}
      </Alert>
    );
  }
  const data = report.data;
  const time = new Intl.DateTimeFormat(i18n.resolvedLanguage ?? i18n.language, { dateStyle: "medium", timeStyle: "short" });
  const last = data.lastComplete;
  return (
    <div className="flex flex-col gap-3" data-testid="source-report">
      <dl className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-4">
        <Count label={t("pipelines.source.atSource")} hint={last ? t("pipelines.source.atSourceHint", { run: last.run }) : t("pipelines.source.noCompleteRun")}>
          {last ? last.read : "–"}
        </Count>
        <Count label={t("pipelines.source.inSpace")} hint={t("pipelines.source.inSpaceHint", { space: data.space ?? "–" })}>
          {data.ledger}
        </Count>
        <Count label={t("pipelines.source.orphans")} hint={t("pipelines.source.orphansHint", { misses: data.misses })}>
          {data.orphans > 0 ? <Badge tone="warning">{data.orphans}</Badge> : 0}
        </Count>
        <Count label={t("pipelines.source.missing")} hint={t("pipelines.source.missingHint")}>
          {data.missing > 0 ? <Badge tone="warning">{data.missing}</Badge> : 0}
        </Count>
      </dl>
      {last ? (
        <p className="text-caption text-fg-muted">
          {t("pipelines.source.lastComplete", { run: last.run })}{" "}
          <time dateTime={last.at}>{time.format(new Date(last.at))}</time>
        </p>
      ) : null}
      {data.idsHidden ? <Alert tone="info">{t("pipelines.source.idsHidden", { space: data.space ?? "–" })}</Alert> : null}
      {data.hold ? (
        <Alert tone="warning" title={t("pipelines.source.held", { run: data.hold.run })}>
          <p>{t(`pipelines.source.holdReason.${data.hold.reason}`, { would: data.hold.would, ledger: data.hold.ledger })}</p>
          <div className="mt-2 flex flex-wrap gap-2">
            <PermissionGuard project={project} kind="Pipeline" verb="approve">
              <Button size="sm" variant="secondary" disabled={decide.isPending} disabledReason={t("app.loading")} onClick={() => decide.mutate("keep")}>
                {t("pipelines.source.keep")}
              </Button>
            </PermissionGuard>
            {data.due.length > 0 ? (
              <PermissionGuard project={project} kind="Pipeline" verb="approve">
                <Button size="sm" variant="danger" disabled={decide.isPending} disabledReason={t("app.loading")} onClick={() => decide.mutate("delete")}>
                  {t("pipelines.source.release", { count: data.due.length })}
                </Button>
              </PermissionGuard>
            ) : null}
          </div>
        </Alert>
      ) : null}
      {refused ? <Alert tone="danger">{refused}</Alert> : null}
      <Ids title={t("pipelines.source.orphanList", { count: data.orphans })} ids={data.orphanIds} total={data.idsHidden ? 0 : data.orphans} testId="source-orphans" />
      <Ids title={t("pipelines.source.missingList", { count: data.missing })} ids={data.missingIds} total={data.idsHidden ? 0 : data.missing} testId="source-missing" />
      {data.idsHidden ? null : <h3 className="text-body font-semibold text-fg">{t("pipelines.source.removed")}</h3>}
      {data.idsHidden ? null : data.removed.length === 0 ? (
        <p className="text-sm text-fg-subtle">{t("pipelines.source.noneRemoved", { misses: data.misses })}</p>
      ) : (
        // The ids keep one line; the Table's own frame scrolls at phone width, by keyboard too.
        <Table caption={t("pipelines.source.removed")}>
          <TableHead>
            <TableHeaderCell>{t("pipelines.source.entity")}</TableHeaderCell>
            <TableHeaderCell>{t("pipelines.source.when")}</TableHeaderCell>
            <TableHeaderCell>{t("pipelines.source.why")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {data.removed.map((r) => (
              <TableRow key={r.id}>
                <TableCell className="whitespace-nowrap font-mono">{r.id}</TableCell>
                <TableCell className="whitespace-nowrap">
                  <time dateTime={r.at}>{time.format(new Date(r.at))}</time>
                </TableCell>
                <TableCell className="whitespace-nowrap">{t("pipelines.source.removedAtSource", { pipeline: r.pipeline, run: r.run })}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      <div>
        <Button
          size="sm"
          variant="secondary"
          disabled={data.idsHidden}
          disabledReason={t("pipelines.source.idsHidden", { space: data.space ?? "–" })}
          onClick={() => download(sourceCsv(data), `${name}-source-vs-space.csv`, "text/csv;charset=utf-8")}
        >
          {t("pipelines.source.export")}
        </Button>
      </div>
    </div>
  );
}

/**
 * The space's tombstones from source deletes (T-4892): each entity a snapshot pipeline deleted
 * because its source no longer listed it, "Removed at the source (pipeline, run)" with the time.
 * Ids only; nothing shown when there is none or the list cannot be read.
 */
export function SourceRemovalsPanel({ project, space }: { project: string; space: string }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const removals = useQuery({
    queryKey: ["projects", project, "spaces", space, "source-removals"],
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/spaces/{space}/source-removals", {
          params: { path: { project, space } },
        }),
      ),
  });
  // An answer of another shape is no list: the panel stays away rather than break the page.
  if (!Array.isArray(removals.data) || removals.data.length === 0) return null;
  const time = new Intl.DateTimeFormat(i18n.resolvedLanguage ?? i18n.language, { dateStyle: "medium", timeStyle: "short" });
  return (
    <details className="rounded-lg border border-border p-3" data-testid="source-removals">
      <summary className="cursor-pointer text-body font-semibold text-fg">
        {t("pipelines.source.spaceRemovals", { count: removals.data.length })}
      </summary>
      <p className="mt-1 text-caption text-fg-muted">{t("pipelines.source.spaceRemovalsLead")}</p>
      <ul className="mt-2 flex flex-col gap-2">
        {removals.data.map((r) => (
          <li key={r.id} className="flex flex-col gap-0.5">
            <span className="font-mono text-caption text-fg [overflow-wrap:anywhere]">{r.id}</span>
            <span className="text-caption text-fg-muted">
              {t("pipelines.source.removedAtSource", { pipeline: r.pipeline, run: r.run })},{" "}
              <time dateTime={r.at}>{time.format(new Date(r.at))}</time>
            </span>
          </li>
        ))}
      </ul>
    </details>
  );
}
