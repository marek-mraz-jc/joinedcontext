import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { liveLabelOf } from "./PipelineFlow";
import type { FlowNodeId } from "./PipelineFlow";
import { RecordView } from "../../components/ui";

/** How many of the kept samples the drawer shows; the route keeps up to 20 per node. */
const SHOWN = 5;

/**
 * The last messages the running stream sent through one node (T-3974), newest first, as the Portal
 * keeps them: without what the target Endpoint hides and with secrets masked. A pipeline that has
 * not run says so; one the reader may not see answers 404 and the drawer says it is not available.
 * A step before the mapping says why it shows nothing when the target hides attributes.
 */
export function LastSamples({
  project,
  pipeline,
  node,
}: {
  project: string;
  pipeline: string;
  node: FlowNodeId;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const samples = useQuery({
    queryKey: [...queryKeys.resource(project, "pipelines", pipeline), "samples"],
    retry: false,
    refetchInterval: 15_000,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/samples", {
          params: { path: { project, name: pipeline } },
        }),
      ),
  });
  const label = liveLabelOf(node);
  const shown = (samples.data?.nodes?.[label] ?? []).slice(0, SHOWN);
  // Before the mapping a record names fields as the source does, so the route serves only the
  // output when a target Endpoint hides attributes.
  const withheld = samples.data?.withheld === true && label !== "output";
  return (
    <section className="flex flex-col gap-1 rounded-md border border-border bg-surface p-2" data-testid="flow-last-samples">
      <span className="text-caption font-semibold text-fg">{t("pipelines.flow.lastSamples")}</span>
      {samples.isPending ? null : samples.isError ? (
        <p className="text-caption text-fg-muted">{t("pipelines.flow.lastSamplesUnavailable")}</p>
      ) : withheld ? (
        <p className="text-caption text-fg-muted">{t("pipelines.flow.lastSamplesWithheld")}</p>
      ) : shown.length === 0 ? (
        <p className="text-caption text-fg-muted">{t("pipelines.flow.lastSamplesNone")}</p>
      ) : (
        <ol className="flex flex-col gap-1">
          {shown.map((sample) => (
            <li key={sample.at} className="flex flex-col gap-0.5">
              <time className="text-caption text-fg-muted" dateTime={sample.at}>
                {new Date(sample.at).toLocaleString(i18n.language)}
              </time>
              <RecordView value={sample.record} label={t("codeBlock.sample")} className="max-h-36" />
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
