import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { FormHeading } from "../../components/forms/FormRoute";
import { Alert, Badge, Button, Card } from "../../components/ui";
import { RejectedGroups } from "./PipelineRejected";

type Checked = components["schemas"]["SourceCheck"];

/** How many rejected records the list under the groups shows before it says how many more. */
const ROWS_SHOWN = 50;

/**
 * The workbench's whole-source check (G-21, T-7085): the saved pipeline over every page of its
 * source, nothing written, counted by the rule each rejected record broke, the same counts a
 * live run's rejected list would hold. It reads the saved pipeline, so a pipeline not yet merged
 * says so instead.
 */
export function SourceCheck({ project, pipeline }: { project: string; pipeline?: string }): JSX.Element {
  const { t } = useTranslation();
  const check = useMutation({
    mutationFn: async (name: string): Promise<Checked> =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/pipelines/{name}/source-check", {
          params: { path: { project, name } },
        }),
      ),
  });
  const answer = check.data;
  return (
    <Card as="section" padding="md" aria-labelledby="source-check-title" className="flex flex-col gap-3">
      <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <FormHeading id="source-check-title" className="text-body font-semibold text-fg">
            {t("pipelines.workbench.sourceCheck.title")}
          </FormHeading>
          <p className="text-caption text-fg-muted">{t("pipelines.workbench.sourceCheck.hint")}</p>
        </div>
        <div className="shrink-0">
          <Button
            size="sm"
            disabled={!pipeline || check.isPending}
            disabledReason={
              !pipeline
                ? t("pipelines.workbench.sourceCheck.unsaved")
                : check.isPending
                  ? t("pipelines.workbench.sourceCheck.running")
                  : undefined
            }
            onClick={() => pipeline && check.mutate(pipeline)}
          >
            {answer ? t("pipelines.workbench.sourceCheck.again") : t("pipelines.workbench.sourceCheck.run")}
          </Button>
        </div>
      </div>
      {!pipeline ? (
        <p className="text-caption text-fg-subtle">{t("pipelines.workbench.sourceCheck.unsaved")}</p>
      ) : check.isPending ? (
        <p role="status" className="text-caption text-fg-subtle">
          {t("pipelines.workbench.sourceCheck.running")}
        </p>
      ) : check.isError ? (
        <Alert role="alert" tone="danger" title={t("pipelines.workbench.sourceCheck.failed")}>
          {check.error instanceof Error ? check.error.message : t("pipelines.workbench.failed")}
        </Alert>
      ) : answer ? (
        <SourceCheckAnswer answer={answer} />
      ) : null}
    </Card>
  );
}

function SourceCheckAnswer({ answer }: { answer: Checked }): JSX.Element {
  const { t } = useTranslation();
  const shown = answer.rows.slice(0, ROWS_SHOWN);
  return (
    <div className="flex flex-col gap-3">
      <Alert
        role="status"
        tone={answer.records === 0 || !answer.complete ? "warning" : answer.rejected === 0 ? "success" : "warning"}
        title={t("pipelines.workbench.sourceCheck.counts", {
          count: answer.records,
          accepted: answer.accepted,
          rejected: answer.rejected,
        })}
      >
        {answer.complete
          ? t("pipelines.workbench.sourceCheck.complete")
          : t("pipelines.workbench.sourceCheck.partial")}
      </Alert>
      {answer.errors.length > 0 ? (
        <ul className="flex flex-col gap-1">
          {answer.errors.map((error) => (
            <li key={error} className="break-words text-body text-fg">
              {error}
            </li>
          ))}
        </ul>
      ) : null}
      {answer.groups.length > 0 ? (
        <RejectedGroups groups={answer.groups} headingId="source-check-groups" />
      ) : answer.records > 0 ? (
        <p className="text-caption text-fg-subtle">{t("pipelines.workbench.sourceCheck.noneRejected")}</p>
      ) : null}
      {shown.length > 0 ? (
        <details className="rounded border border-border">
          <summary className="flex min-h-11 cursor-pointer items-center p-3 text-body text-fg">
            {t("pipelines.workbench.sourceCheck.rows", { count: answer.rows.length })}
          </summary>
          <ol className="flex max-h-96 flex-col gap-2 overflow-y-auto px-3 pb-3">
            {shown.map((row) => (
              <li key={row.id} className="flex min-w-0 flex-col gap-1 border-t border-border pt-2">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-caption tabular-nums text-fg-subtle">
                    {t("pipelines.workbench.sourceCheck.place", { place: row.id })}
                  </span>
                  <Badge mono tone="danger">
                    {row.rule}
                  </Badge>
                  {row.path ? <span className="font-mono text-caption text-fg">{row.path}</span> : null}
                </div>
                <p className="break-words text-body text-fg">{row.message}</p>
              </li>
            ))}
          </ol>
          {answer.rows.length > shown.length ? (
            <p className="px-3 pb-3 text-caption text-fg-subtle">
              {t("pipelines.workbench.sourceCheck.more", { count: answer.rows.length - shown.length })}
            </p>
          ) : null}
        </details>
      ) : null}
    </div>
  );
}
