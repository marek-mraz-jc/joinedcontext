import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { usePermissions } from "../../api/permissions";
import { download } from "../models/diagramExport";
import { RecordView,
  Alert,
  Badge,
  Button,
  Checkbox,
  Dialog,
  EmptyState,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "../../components/ui";

type Rejected = components["schemas"]["Rejected"];
type RejectedGroup = components["schemas"]["RejectedGroupView"];
type RetryAnswer = components["schemas"]["RetryAnswer"];

const PAGE = 50;
const COLUMNS = 4;

/**
 * The records a pipeline's validation stage refused and did not write, newest first, each with
 * the rule it broke (PL-61, ADR-N-034). A person fixes the mapping or the model, lets the change
 * merge, ticks the records and replays them: what passes now is written, what still breaks the
 * model comes back with its rule. A record whose secret was masked when it was kept is not
 * replayed, because the mask would be written; the answer names those.
 */
export function PipelineRejectedDialog({
  project,
  name,
  onClose,
  onOpenRun,
}: {
  project: string;
  name: string;
  onClose: () => void;
  /** Opens the log of the run a record was refused in (T-3252). */
  onOpenRun?: (run: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const permissions = usePermissions(project);
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const time = new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" });
  // The pages walked so far, as their `before`: the first is the newest page.
  const [cursors, setCursors] = useState<(number | undefined)[]>([undefined]);
  const before = cursors[cursors.length - 1];
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [answer, setAnswer] = useState<RetryAnswer | null>(null);
  const [error, setError] = useState<string | null>(null);

  const listKey = [...queryKeys.resource(project, "pipelines", name), "rejected"];
  const page = useQuery({
    queryKey: [...listKey, before ?? "newest"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/rejected", {
          params: { path: { project, name }, query: { limit: PAGE, before } },
        }),
      ),
  });

  const retry = useMutation({
    mutationFn: async (ids: number[]) => {
      setError(null);
      setAnswer(null);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/pipelines/{name}/rejected/retry", {
          params: { path: { project, name } },
          body: { ids },
        }),
      );
    },
    onSuccess: (result) => {
      setAnswer(result);
      setSelected(new Set());
      setCursors([undefined]);
      void queryClient.invalidateQueries({ queryKey: listKey });
    },
    onError: (err) => {
      setError(err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic"));
    },
  });

  // Every kept record counted by rule and path on the server: a page of 50 would miscount (G-21).
  const groups = useQuery({
    queryKey: [...listKey, "groups"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/rejected/groups", {
          params: { path: { project, name } },
        }),
      ),
  });
  const exported = useMutation({
    mutationFn: async () => {
      const csv: unknown = await unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/rejected/export", {
          params: { path: { project, name } },
          parseAs: "text",
        }),
      );
      download(typeof csv === "string" ? csv : "", `${project}-${name}-rejected.csv`, "text/csv;charset=utf-8");
    },
  });

  const items: Rejected[] = page.data?.items ?? [];
  const toggle = (id: number) =>
    setSelected((current) => {
      const next = new Set(current);
      if (!next.delete(id)) {
        next.add(id);
      }
      return next;
    });
  const allOnPage = items.length > 0 && items.every((item) => selected.has(item.id));
  const listed = page.isSuccess && (items.length > 0 || before !== undefined);
  const mayRetry = permissions.can("Pipeline", "propose");
  const retryRefusal = !mayRetry
    ? t("permissions.denied", { verb: "propose", kind: "Pipeline" })
    : selected.size === 0
      ? t("pipelines.rejected.pickFirst")
      : selected.size > 100
        ? t("pipelines.rejected.tooMany")
        : undefined;

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) {
          onClose();
        }
      }}
      title={t("pipelines.rejected.title", { name })}
      description={t("pipelines.rejected.lead")}
      size="xl"
      closeLabel={t("pipelines.rejected.close")}
      // Retry only beside records: an empty or unread list has nothing to retry (T-4240).
      footer={
        listed ? (
          <Button
            variant="primary"
            disabled={retryRefusal !== undefined || retry.isPending}
            disabledReason={retryRefusal}
            onClick={() => retry.mutate([...selected])}
          >
            {retry.isPending
              ? t("pipelines.rejected.retrying")
              : t("pipelines.rejected.retry", { count: selected.size })}
          </Button>
        ) : undefined
      }
    >
      <div className="flex flex-col gap-4">
        {answer ? (
          <Alert role="status" tone={answer.masked.length > 0 ? "warning" : "success"}>
            {t("pipelines.rejected.replayed", { count: answer.replayed })}
            {answer.masked.length > 0
              ? ` ${t("pipelines.rejected.maskedKept", { count: answer.masked.length })}`
              : ""}
          </Alert>
        ) : null}
        {error ? (
          <Alert role="alert" tone="danger">
            {error}
          </Alert>
        ) : null}
        {groups.data && groups.data.groups.length > 0 ? (
          <RejectedGroups
            groups={groups.data.groups}
            exporting={exported.isPending}
            exportError={
              exported.error
                ? exported.error instanceof ApiError
                  ? (exported.error.problem?.detail ?? exported.error.message)
                  : t("app.error.generic")
                : null
            }
            onExport={() => exported.mutate()}
            fileName={`${project}-${name}-rejected.csv`}
          />
        ) : null}
        {page.isError ? (
          <Alert role="alert" tone="danger">
            {page.error instanceof ApiError ? page.error.message : t("app.error.generic")}
          </Alert>
        ) : page.isPending ? (
          <Table caption={t("pipelines.rejected.caption")} status={t("app.loading")}>
            <TableSkeleton columns={COLUMNS} />
          </Table>
        ) : items.length === 0 && before === undefined ? (
          <EmptyState
            bare
            icon="pipelines"
            title={t("pipelines.rejected.empty")}
            description={t("pipelines.rejected.emptyHint")}
          />
        ) : (
          <>
            <p className="text-caption text-fg-subtle">
              {t("pipelines.rejected.total", { count: page.data.total })}
            </p>
            <Table caption={t("pipelines.rejected.caption")} maxHeight="max-h-96">
              <TableHead>
                <TableHeaderCell>
                  <Checkbox
                    label={<span className="sr-only">{t("pipelines.rejected.pickPage")}</span>}
                    checked={allOnPage}
                    onChange={() =>
                      setSelected((current) => {
                        const next = new Set(current);
                        for (const item of items) {
                          if (allOnPage) {
                            next.delete(item.id);
                          } else {
                            next.add(item.id);
                          }
                        }
                        return next;
                      })
                    }
                  />
                </TableHeaderCell>
                <TableHeaderCell>{t("pipelines.rejected.at")}</TableHeaderCell>
                <TableHeaderCell>{t("pipelines.rejected.rule")}</TableHeaderCell>
                <TableHeaderCell>{t("pipelines.rejected.reason")}</TableHeaderCell>
              </TableHead>
              <TableBody>
                {items.map((item) => (
                  <TableRow key={item.id}>
                    <TableCell>
                      <Checkbox
                        label={
                          <span className="sr-only">
                            {t("pipelines.rejected.pick", { at: time.format(new Date(item.at)) })}
                          </span>
                        }
                        checked={selected.has(item.id)}
                        onChange={() => toggle(item.id)}
                      />
                    </TableCell>
                    <TableCell>
                      <time dateTime={item.at} className="whitespace-nowrap tabular-nums">
                        {time.format(new Date(item.at))}
                      </time>
                    </TableCell>
                    <TableCell>
                      <div className="flex flex-col items-start gap-1">
                        <Badge mono tone="danger">
                          {item.rule}
                        </Badge>
                        {item.path ? (
                          <span className="font-mono text-caption text-fg-subtle">{item.path}</span>
                        ) : null}
                        {item.step != null ? (
                          <span className="text-caption text-fg-subtle">
                            {t("pipelines.rejected.step", { step: item.step + 1 })}
                          </span>
                        ) : null}
                        {item.run && onOpenRun ? (
                          <Button size="sm" variant="ghost" onClick={() => onOpenRun(item.run as string)}>
                            {t("pipelines.rejected.openRun")}
                          </Button>
                        ) : null}
                      </div>
                    </TableCell>
                    <TableCell>
                      <p className="text-body text-fg">{item.message}</p>
                      <details className="mt-1">
                        <summary className="cursor-pointer text-caption text-fg-subtle">
                          {t("pipelines.rejected.record")}
                        </summary>
                        <RecordView value={item.record} label={t("pipelines.rejected.record")} className="mt-1 max-h-64 rounded bg-surface-subtle p-2" />
                      </details>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            <div className="flex flex-wrap items-center gap-2">
              {cursors.length > 1 ? (
                <Button size="sm" onClick={() => setCursors([undefined])}>
                  {t("pipelines.rejected.newest")}
                </Button>
              ) : null}
              {page.data.next != null ? (
                <Button
                  size="sm"
                  onClick={() => setCursors((current) => [...current, page.data.next ?? undefined])}
                >
                  {t("pipelines.rejected.older")}
                </Button>
              ) : null}
              {selected.size > 0 ? (
                <span className="text-caption text-fg-subtle" aria-live="polite">
                  {t("pipelines.rejected.picked", { count: selected.size })}
                </span>
              ) : null}
            </div>
          </>
        )}
      </div>
    </Dialog>
  );
}

/**
 * The rejected records counted by the rule they broke and where (G-21), the largest group first:
 * what to fix first, whether the fix belongs in the data model or in the mapping, and for an enum
 * the values the model allows. The export takes every kept record to whoever owns the source.
 */
function RejectedGroups({
  groups,
  exporting,
  exportError,
  onExport,
  fileName,
}: {
  groups: RejectedGroup[];
  exporting: boolean;
  exportError: string | null;
  onExport: () => void;
  fileName: string;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <section aria-labelledby="rejected-groups" className="flex flex-col gap-3">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <h3 id="rejected-groups" className="text-body font-semibold text-fg">
            {t("pipelines.rejected.groups.title")}
          </h3>
          <p className="text-caption text-fg-subtle">{t("pipelines.rejected.groups.lead")}</p>
        </div>
        <div className="shrink-0">
          <Button
            size="sm"
            disabled={exporting}
            disabledReason={exporting ? t("pipelines.rejected.groups.exporting") : undefined}
            onClick={onExport}
          >
            {exporting ? t("pipelines.rejected.groups.exporting") : t("pipelines.rejected.groups.export")}
          </Button>
        </div>
      </div>
      <p className="text-caption text-fg-subtle">
        {t("pipelines.rejected.groups.exportHint", { file: fileName })}
      </p>
      {exportError ? (
        <Alert role="alert" tone="danger">
          {exportError}
        </Alert>
      ) : null}
      <ul className="flex flex-col gap-2">
        {groups.map((group, index) => (
          <li
            key={`${group.rule}\u0000${group.path}`}
            className="min-w-0 rounded border border-border bg-surface"
          >
            {/* The largest group opens; the rest stay one line each, so the records stay in view. */}
            <details open={index === 0} className="group">
              <summary className="flex min-h-11 cursor-pointer list-none flex-wrap items-center gap-2 rounded p-3 hover:bg-surface-subtle [&::-webkit-details-marker]:hidden">
                <span
                  aria-hidden="true"
                  className="inline-block text-fg transition-transform group-open:rotate-90"
                >
                  ▸
                </span>
                <Badge mono tone="danger">
                  {group.rule}
                </Badge>
                {group.path ? <span className="font-mono text-caption text-fg">{group.path}</span> : null}
                <span className="ml-auto text-caption font-semibold tabular-nums text-fg">
                  {t("pipelines.rejected.groups.count", { count: group.count })}
                </span>
              </summary>
              <div className="flex flex-col gap-2 px-3 pb-3">
                <p className="text-body text-fg">{t(`pipelines.rejected.groups.fix.${group.fix}`)}</p>
                {group.allowed && group.allowed.length > 0 ? (
                  <p className="text-body text-fg">
                    {t("pipelines.rejected.groups.allowed")}{" "}
                    <span className="font-mono">{group.allowed.join(", ")}</span>
                  </p>
                ) : null}
                {group.examples[0] ? (
                  <figure className="border-l-2 border-border pl-2">
                    <figcaption className="text-caption text-fg-subtle">
                      {t("pipelines.rejected.groups.example")}
                    </figcaption>
                    <p className="break-words text-body text-fg">{group.examples[0].message}</p>
                  </figure>
                ) : null}
              </div>
            </details>
          </li>
        ))}
      </ul>
    </section>
  );
}
