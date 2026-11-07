import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { useAuth } from "../../auth/AuthProvider";
import { Link } from "@tanstack/react-router";
import { Alert, Badge, Button, Card, ExternalLink, PageHeader, PageLoading } from "../../components/ui";
import { ResourcePageFailed } from "../../components/ui/PageState";
import { copyIntoPreview, PER_TYPE } from "./copyIntoPreview";
import type { CopyResult } from "./copyIntoPreview";

const POLL_MS = 5000;

/**
 * Try it (UI-61, CC-78, PF-83): the copy's preview, where its Endpoints answer, the pipelines
 * that stay paused, and the person's own bounded copy of real data into it.
 */
export function TryItPage({ project, name }: { project: string; name: string }): React.JSX.Element {
  const { t } = useTranslation();
  const { identity } = useAuth();
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const [copies, setCopies] = useState<Record<string, CopyResult>>({});
  const path = { project, name };
  const key = [...queryKeys.list(project, "workspaces"), name, "preview"];

  const workspace = useQuery({
    queryKey: [...queryKeys.list(project, "workspaces"), name],
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/workspaces/{name}", { params: { path } })),
    retry: false,
  });
  const preview = useQuery({
    queryKey: key,
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/workspaces/{name}/preview", { params: { path } })),
    refetchInterval: (query) => (query.state.data?.state === "starting" ? POLL_MS : false),
    retry: false,
  });

  const settled = {
    onSuccess: () => setError(null),
    onError: (err: Error) => setError(err.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: key }),
  };
  const start = useMutation({
    mutationFn: async () =>
      unwrap(await api.POST("/api/v1/projects/{project}/workspaces/{name}/preview", { params: { path } })),
    ...settled,
  });
  const stop = useMutation({
    mutationFn: async () =>
      unwrap(await api.DELETE("/api/v1/projects/{project}/workspaces/{name}/preview", { params: { path } })),
    ...settled,
  });
  const copy = useMutation({
    mutationFn: async (endpoint: { name: string; slug: string; originSlug: string }) => ({
      name: endpoint.name,
      result: await copyIntoPreview(endpoint.originSlug, endpoint.slug),
    }),
    onSuccess: ({ name: endpoint, result }) => setCopies((held) => ({ ...held, [endpoint]: result })),
    onError: (err: Error) => setError(err.message),
  });

  // Reading the copy and reading its preview are two requests that can fail on their own, and
  // until this was here both failures were drawn as "no preview": someone refused the copy saw
  // the same page as someone whose preview is simply not running, with a Start button that was
  // going to be refused too (UI-15, UI-44).
  if (workspace.isPending || preview.isPending) {
    return (
      <div className="space-y-6">
        <PageHeader title={t("workspaces.tryIt.title")} description={t("workspaces.tryIt.lead", { name })} />
        <PageLoading label={t("app.loading")} />
      </div>
    );
  }
  if (workspace.isError || preview.isError) {
    const failed = workspace.isError ? workspace : preview;
    return (
      <ResourcePageFailed
        title={t("workspaces.tryIt.title")}
        description={t("workspaces.tryIt.lead", { name })}
        error={failed.error}
        onRetry={() => {
          void failed.refetch();
        }}
        back={
          <Link to="/projects/$project/workspaces" params={{ project }} className="focus-ring text-body text-primary-soft-fg underline hover:no-underline">
            {t("workspaces.compare.back")}
          </Link>
        }
      />
    );
  }

  const owner = workspace.data?.owner;
  const mine = !!identity && !!owner && (owner === identity.email || owner === identity.username);
  const state = preview.data?.state ?? "none";
  const running = state === "running" || state === "starting";
  const tone = state === "running" ? "success" : state === "error" ? "danger" : "neutral";

  return (
    <div className="space-y-6">
      <PageHeader
        title={t("workspaces.tryIt.title")}
        description={t("workspaces.tryIt.lead", { name: workspace.data?.title ?? name })}
      />
      <div className="flex flex-wrap items-center gap-3">
        <Badge tone={tone} data-testid="preview-state">
          {t(`workspaces.previewStates.${state}`)}
        </Badge>
        {mine ? (
          running ? (
            <Button variant="secondary" onClick={() => stop.mutate()} disabled={stop.isPending} disabledReason={t("app.busyReason")}>
              {t("workspaces.tryIt.stop")}
            </Button>
          ) : (
            <Button onClick={() => start.mutate()} disabled={start.isPending || preview.isLoading} disabledReason={t("app.busyReason")}>
              {t("workspaces.tryIt.start")}
            </Button>
          )
        ) : (
          <span className="text-body text-fg-muted">{t("workspaces.tryIt.notOwner")}</span>
        )}
      </div>
      {error ? <Alert tone="danger">{error}</Alert> : null}
      {state === "error" && preview.data?.reason ? (
        <Alert tone="danger" title={t("workspaces.tryIt.failed")}>
          {preview.data.reason}
        </Alert>
      ) : null}

      {state === "running" ? (
        <>
          <section aria-labelledby="preview-addresses">
            <h2 id="preview-addresses" className="mb-2 text-body font-semibold text-fg">
              {t("workspaces.tryIt.addresses")}
            </h2>
            {preview.data?.endpoints.length ? (
              <ul className="space-y-3">
                {preview.data.endpoints.map((endpoint) => {
                  const done = copies[endpoint.name];
                  const total = done ? Object.values(done.copied).reduce((a, b) => a + b, 0) : 0;
                  return (
                    <Card as="li" padding="sm" key={endpoint.name}>
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-medium text-fg">{endpoint.name}</span>
                        <ExternalLink className="break-all font-mono text-body" href={endpoint.url}>
                          {endpoint.url}
                        </ExternalLink>
                      </div>
                      {mine && endpoint.originSlug ? (
                        <div className="mt-2 flex flex-wrap items-center gap-3 text-body">
                          <Button
                            size="sm"
                            variant="secondary"
                            disabled={copy.isPending}
                            disabledReason={t("app.busyReason")}
                            onClick={() =>
                              copy.mutate({
                                name: endpoint.name,
                                slug: endpoint.slug,
                                originSlug: endpoint.originSlug!,
                              })
                            }
                          >
                            {t("workspaces.tryIt.copyData")}
                          </Button>
                          <span className="text-fg-muted">{t("workspaces.tryIt.copyHint", { limit: PER_TYPE })}</span>
                        </div>
                      ) : null}
                      {done ? (
                        <p className="mt-2 text-body" role="status">
                          {t("workspaces.tryIt.copied", { count: total })}
                          {done.stopped ? ` ${t("workspaces.tryIt.copyStopped", { reason: done.stopped })}` : ""}
                        </p>
                      ) : null}
                    </Card>
                  );
                })}
              </ul>
            ) : (
              <p className="text-body text-fg-muted">{t("workspaces.tryIt.noEndpoints")}</p>
            )}
          </section>
          <section aria-labelledby="preview-paused">
            <h2 id="preview-paused" className="mb-2 text-body font-semibold text-fg">
              {t("workspaces.tryIt.pausedPipelines")}
            </h2>
            {preview.data?.pausedPipelines.length ? (
              <ul className="list-disc pl-5 text-body">
                {preview.data.pausedPipelines.map((pipeline) => (
                  <li key={pipeline}>{pipeline}</li>
                ))}
              </ul>
            ) : (
              <p className="text-body text-fg-muted">{t("workspaces.tryIt.noPipelines")}</p>
            )}
          </section>
        </>
      ) : null}
    </div>
  );
}
