import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";
import { RecordLink } from "../../components/RecordLink";
import { RecordView, Button } from "../../components/ui";
import { readsOf } from "../mqttbrokers/connections";

/** What the server read from the source (`api/datasource_test.rs`), or why it read nothing. */
interface Probe {
  records?: number;
  bytes?: number;
  sample?: unknown;
  skipped?: string;
}

/**
 * Test connection and Used by for one saved data source (T-4819, MF-39). The test runs the saved
 * source on the project's runner, server-side under the project's grants, so the credential stays
 * a `secretRef` the runner fills and the answer holds only what the source sent.
 */
export function ConnectionTest({ project, name }: { project: string; name: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const test = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/datasources/{name}/test", {
          params: { path: { project, name } },
        }),
      ) as Probe,
  });
  const pipelines = useQuery({
    queryKey: queryKeys.list(project, "pipelines"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "pipelines" } },
        }),
      ),
  });
  const readers = asManifests(pipelines.data?.items ?? []).filter((pipeline) =>
    readsOf((pipeline.spec ?? {}) as Record<string, unknown>).includes(name),
  );
  const failure =
    test.error instanceof ApiError ? (test.error.problem?.detail ?? test.error.message) : test.error ? t("app.error.generic") : null;
  const probe = test.data;

  return (
    <section aria-labelledby="datasource-test-heading" className="flex flex-col gap-3" data-testid="datasource-connection">
      <h2 id="datasource-test-heading" className="text-body font-medium">
        {t("datasources.test.title")}
      </h2>
      <p className="text-caption text-fg-muted">{t("datasources.test.hint")}</p>
      <div>
        <Button type="button" variant="secondary" size="sm" loading={test.isPending} onClick={() => test.mutate()}>
          {test.isPending ? t("datasources.test.running") : t("datasources.test.button")}
        </Button>
      </div>
      <div role="status" aria-live="polite">
        {failure ? <p className="text-body text-danger-fg">{failure}</p> : null}
        {probe?.skipped ? (
          <p className="text-body text-danger-fg">{t("datasources.test.failed", { reason: probe.skipped })}</p>
        ) : probe ? (
          <>
            <p className="text-body">{t("datasources.test.read", { records: probe.records ?? 0, bytes: probe.bytes ?? 0 })}</p>
            {probe.sample !== undefined ? (
              <RecordView value={probe.sample} label={t("codeBlock.sample")} className="mt-2 max-h-60 rounded-md bg-surface-subtle p-2" />
            ) : null}
          </>
        ) : null}
      </div>
      <h2 className="text-body font-medium">{t("datasources.usedBy.title")}</h2>
      {readers.length > 0 ? (
        <ul className="flex flex-col gap-1">
          {readers.map((pipeline) => (
            <li key={pipeline.metadata.name}>
              <RecordLink project={project} plural="pipelines" name={pipeline.metadata.name}>
                {localized(pipeline.metadata.title, locale, pipeline.metadata.name)}
              </RecordLink>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-caption text-fg-muted">
          {pipelines.isPending ? t("app.loading") : t("datasources.usedBy.none")}
        </p>
      )}
    </section>
  );
}
