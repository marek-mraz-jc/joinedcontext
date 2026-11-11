import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { Manifest } from "../../api/manifest";
import { useDebounced } from "../../components/pickers/organizationModels";
import {
  Alert,
  Badge,
  EmptyState,
  PageFailed,
} from "../../components/ui";
import { AdminGrid } from "../../components/grid/AdminGrid";
import type { DataColumn } from "../../components/grid/AdminGrid";
import { previewContext } from "./knowledge";
import type { AssistantContext } from "./knowledge";

/** The editor's live preview: what the manifest being edited will let the assistant read (AG-121, T-4693). */
export function AssistantContextPreview({ project, manifest }: { project: string; manifest: Manifest }): JSX.Element {
  const { t, i18n } = useTranslation();
  const lang = i18n.language.slice(0, 2);
  // The form changes on every key; the preview follows once typing pauses.
  const settled = useDebounced(JSON.stringify(manifest), 400);
  const preview = useQuery({
    queryKey: ["projects", project, "knowledge", "deployments", "context", lang, settled],
    queryFn: () => previewContext(project, JSON.parse(settled) as Manifest, lang),
    placeholderData: (previous) => previous,
    retry: false,
  });
  return (
    <section aria-labelledby="assistant-context-heading" className="space-y-3">
      <h2 id="assistant-context-heading" className="text-base font-semibold">
        {t("knowledge.context.heading")}
      </h2>
      <div aria-live="polite" aria-busy={preview.isFetching}>
        {preview.isError ? (
          <PageFailed
            error={preview.error}
            onRetry={() => {
              void preview.refetch();
            }}
          />
        ) : preview.data ? (
          <ContextView context={preview.data} />
        ) : (
          <p role="status">{t("app.loading")}</p>
        )}
      </div>
    </section>
  );
}

/** The preview's rows are already in hand: nothing is loading or failing inside the grid. */
const LOADED = { isPending: false, isError: false, error: null, refetch: () => undefined };

/** One resolved context: categories, the Endpoints and the sources, and what is missing. */
export function ContextView({ context }: { context: AssistantContext }): JSX.Element {
  const { t } = useTranslation();
  const dash = (items: string[]): string => items.join(", ") || "—";
  const columns: DataColumn<AssistantContext["endpoints"][number]>[] = [
    { key: "endpoint", label: t("knowledge.context.endpoint"), text: (e) => e.title ?? e.name },
    { key: "publisher", label: t("knowledge.context.publisher"), text: (e) => e.project },
    {
      key: "categories",
      label: t("knowledge.context.categories"),
      text: (e) => dash(e.themes.map((theme) => theme.label)),
    },
    { key: "types", label: t("knowledge.context.types"), text: (e) => dash(e.types) },
    {
      key: "how",
      label: t("knowledge.context.how"),
      text: (e) =>
        e.tools.length > 0
          ? t("knowledge.context.connector", { tools: e.tools.join(", ") })
          : t("knowledge.context.selected"),
    },
  ];
  return (
    <div className="space-y-3">
      {context.problems.length > 0 ? (
        <Alert tone="warning" title={t("knowledge.context.problems")}>
          <ul className="list-disc pl-5">
            {context.problems.map((problem) => (
              <li key={problem}>{problem}</li>
            ))}
          </ul>
        </Alert>
      ) : null}
      {context.categories.length > 0 ? (
        <p>
          <span className="mr-2">{t("knowledge.context.categories")}</span>
          {context.categories.map((category) => (
            <Badge key={category.code} tone="info" className="mr-1">
              {category.label}
            </Badge>
          ))}
        </p>
      ) : null}
      {context.endpoints.length === 0 ? (
        <p>{t("knowledge.context.noEndpoints")}</p>
      ) : (
        <AdminGrid<AssistantContext["endpoints"][number]>
          query={LOADED}
          caption={t("knowledge.context.endpoints", { count: context.endpoints.length })}
          rows={context.endpoints}
          columns={columns}
          rowKey={(endpoint) => endpoint.slug}
          storageKey="knowledge:context-endpoints"
          stack
          empty={<EmptyState bare title={t("knowledge.context.noEndpoints")} description={t("knowledge.context.noEndpointsHint")} />}
        />
      )}
      {context.omitted.length > 0 ? (
        <Alert tone="warning" title={t("knowledge.context.omittedTitle", { count: context.omitted.length })}>
          {t("knowledge.context.omitted", { slugs: context.omitted.join(", ") })}
        </Alert>
      ) : null}
      {context.sources.length === 0 ? (
        <p>{t("knowledge.context.noSources")}</p>
      ) : (
        <ul aria-label={t("knowledge.context.sources")} className="list-disc pl-5">
          {context.sources.map((source) => (
            <li key={`${source.project}/${source.name}`}>
              {source.name} ({t(`knowledge.type.${source.kind}`, { defaultValue: source.kind })})
              {source.sites.length > 0 ? `: ${source.sites.join(", ")}` : ""}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
