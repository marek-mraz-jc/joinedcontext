import { useState } from "react";
import type { JSX } from "react";
import { useInfiniteQuery, useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { usePermissions } from "../api/permissions";
import { changeSentence, fieldSentence } from "../routes/changeWords";
import type { PlannedField } from "../routes/changeWords";
import { LifecycleBadge } from "./status/LifecycleBadge";
import { Alert, Button } from "./ui";

/** What one closed change touched, each field a sentence, read when the person asks (T-3274). */
export function ChangedFields({ project, id }: { project: string; id: string }): JSX.Element {
  const { t } = useTranslation();
  const detail = useQuery({
    queryKey: queryKeys.change(project, id),
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/changes/{id}", { params: { path: { project, id } } })),
  });
  if (detail.isPending) return <p className="text-caption text-fg-muted">{t("app.loading")}</p>;
  const fields = (detail.data?.planFields ?? []) as PlannedField[];
  if (detail.isError || fields.length === 0) {
    return <p className="text-caption text-fg-muted">{t("approvals.history.noFields")}</p>;
  }
  return (
    <ul className="mt-1 list-disc pl-5 text-caption text-fg">
      {fields.map((field) => (
        <li key={field.path}>{fieldSentence(field, t)}</li>
      ))}
    </ul>
  );
}

/**
 * One resource's own history inside its editor (T-3975): the closed changes that touched it,
 * newest first, each with who proposed it, when it closed and what it changed. "Restore this
 * version" reads the manifest a merged change wrote and hands it to the editor, which checks and
 * proposes it like any edit; nothing is written here. A person who may not propose the kind
 * sees the history and no restore, and so does a read-only page that passes no `onRestore`.
 */
export function ResourceHistory({
  project,
  kind,
  name,
  onRestore,
}: {
  project: string;
  kind: string;
  name: string;
  onRestore?: (manifest: unknown, change: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const { can } = usePermissions(project);
  const [opened, setOpened] = useState<string | null>(null);
  const history = useInfiniteQuery({
    queryKey: [...queryKeys.changes(project), "history", kind, name, "", "", ""] as const,
    initialPageParam: 1,
    queryFn: async ({ pageParam }) =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/changes/history", {
          params: { path: { project }, query: { page: pageParam, kind, name } },
        }),
      ),
    getNextPageParam: (last) => last.next ?? undefined,
  });
  // The API matches a name that contains the one asked; this resource is the one named exactly.
  const items = (history.data?.pages.flatMap((page) => page.items) ?? []).filter(
    (change) => (change.summary.params as Record<string, unknown>).name === name,
  );
  const restore = useMutation({
    mutationFn: async (id: string) => {
      const list = await unwrap(
        await api.GET("/api/v1/projects/{project}/changes/{id}/manifests", {
          params: { path: { project, id } },
        }),
      );
      const manifest = list.items.find((item) => item.kind === kind && item.metadata.name === name);
      if (!manifest) {
        throw new Error(t("resourceHistory.notWritten", { kind, name }));
      }
      return { manifest, id };
    },
    onSuccess: ({ manifest, id }) => onRestore?.(manifest, id),
  });
  const dateFormatter = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" });

  if (history.isPending) {
    return <p className="text-caption text-fg-muted">{t("app.loading")}</p>;
  }
  if (history.isError) {
    return (
      <Alert tone="danger" role="alert">
        {history.error instanceof ApiError ? (history.error.problem?.detail ?? history.error.message) : t("app.error.generic")}
      </Alert>
    );
  }
  return (
    <div className="flex flex-col gap-3" data-testid="resource-history">
      {restore.error ? (
        <Alert tone="danger" role="alert">
          {restore.error instanceof ApiError
            ? (restore.error.problem?.detail ?? restore.error.message)
            : restore.error.message}
        </Alert>
      ) : null}
      {items.length === 0 ? (
        <p className="text-caption text-fg-muted">
          {history.hasNextPage ? t("resourceHistory.noneOnPage") : t("resourceHistory.empty", { kind, name })}
        </p>
      ) : (
        <ul className="flex flex-col divide-y divide-border rounded-md border border-border">
          {items.map((change) => (
            <li key={change.metadata.name} className="flex flex-col gap-1 p-3" data-testid="resource-history-item">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="text-body text-fg">
                  {changeSentence(change as Parameters<typeof changeSentence>[0], t)}
                </span>
                <LifecycleBadge kind="phase" value={change.status.phase} />
              </div>
              <span className="text-caption text-fg-muted">
                {t("resourceHistory.when", {
                  at: dateFormatter.format(new Date(change.decision?.at ?? change.createdAt)),
                  lane: change.status.lane,
                  id: change.metadata.name,
                })}
              </span>
              <div className="flex flex-wrap gap-2">
                <Button
                  size="sm"
                  variant="ghost"
                  aria-expanded={opened === change.metadata.name}
                  onClick={() => setOpened(opened === change.metadata.name ? null : change.metadata.name)}
                >
                  {t("approvals.history.whatChanged")}
                </Button>
                {onRestore && change.status.phase === "Merged" && can(kind, "propose") ? (
                  <Button
                    size="sm"
                    variant="secondary"
                    data-testid="resource-history-restore"
                    loading={restore.isPending && restore.variables === change.metadata.name}
                    onClick={() => restore.mutate(change.metadata.name)}
                  >
                    {t("resourceHistory.restore")}
                  </Button>
                ) : null}
              </div>
              {opened === change.metadata.name ? <ChangedFields project={project} id={change.metadata.name} /> : null}
            </li>
          ))}
        </ul>
      )}
      {history.hasNextPage ? (
        <div>
          <Button
            size="sm"
            variant="secondary"
            disabled={history.isFetchingNextPage}
            disabledReason={t("app.busyReason")}
            onClick={() => void history.fetchNextPage()}
          >
            {t("approvals.history.older")}
          </Button>
        </div>
      ) : null}
    </div>
  );
}
