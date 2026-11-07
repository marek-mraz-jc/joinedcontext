import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import type { components } from "../../api/schema";
import { Alert, Badge, Card, EmptyState, Icon, PageFailed, PageLoading } from "../../components/ui";
import type { BadgeTone } from "../../components/ui/Badge";
import { readableQuery } from "./queries";

type ReadableDataset = components["schemas"]["ReadableDataset"];

const TONE: Record<string, BadgeTone> = { public: "success", organization: "primary", group: "accent", granted: "info" };

/**
 * What grants a dataset, in words: public, your organization, the group, or you alone. `words`
 * names the locale group, so an administrator's view of another person says "everyone in the
 * organization" instead of "your organization".
 */
export function AccessBadge({
  dataset,
  words = "catalogue.readable.access",
}: {
  dataset: Pick<ReadableDataset, "access" | "group">;
  words?: "catalogue.readable.access" | "dataAccess.personAccess";
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Badge tone={TONE[dataset.access] ?? "neutral"}>
      {t(`${words}.${dataset.access}`, { group: dataset.group ?? "" })}
    </Badge>
  );
}

/**
 * The internal datasets a signed-in person may read (ADR-N-067 §2.4, T-6411): the server asks
 * the gateway per Endpoint and lists only what it grants, so the page shows every row it gets.
 * The public ones are listed by the open-data search below, not here.
 */
export function ReadableDatasets(): JSX.Element {
  const { t } = useTranslation();
  const readable = useQuery(readableQuery());
  const body = (() => {
    if (readable.isPending) {
      return <PageLoading label={t("catalogue.readable.loading")} />;
    }
    if (readable.isError || !Array.isArray(readable.data.datasets)) {
      return <PageFailed error={readable.error} onRetry={() => void readable.refetch()} />;
    }
    const internal = readable.data.datasets.filter((dataset) => dataset.access !== "public");
    return (
      <>
        {readable.data.unanswered > 0 ? (
          <Alert tone="warning">{t("catalogue.readable.unanswered", { count: readable.data.unanswered })}</Alert>
        ) : null}
        {internal.length === 0 ? (
          <EmptyState icon="lock" bare title={t("catalogue.readable.empty.title")} description={t("catalogue.readable.empty.description")} />
        ) : (
          <ul className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
            {internal.map((dataset) => (
              <ReadableRow key={dataset.slug} dataset={dataset} />
            ))}
          </ul>
        )}
      </>
    );
  })();
  return (
    <Card>
      <section aria-labelledby="readable-title" className="flex flex-col gap-3">
        <div>
          <h2 id="readable-title" className="text-title font-semibold text-fg">
            {t("catalogue.readable.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("catalogue.readable.lead")}</p>
        </div>
        {body}
      </section>
    </Card>
  );
}

export function ReadableRow({ dataset }: { dataset: ReadableDataset }): JSX.Element {
  const { t } = useTranslation();
  return (
    <li className="flex min-w-0 flex-col gap-2 rounded-md border border-border p-3 transition-colors hover:border-primary">
      <div className="flex flex-wrap items-center gap-2">
        <Link
          to="/catalogue/endpoint/$slug"
          params={{ slug: dataset.slug }}
          className="focus-ring min-w-0 break-words rounded-sm text-body font-medium text-primary-soft-fg hover:underline"
          aria-label={`${t("catalogue.readable.open")}: ${dataset.title ?? dataset.name}`}
        >
          {dataset.title ?? dataset.name}
          <Icon name="chevronRight" className="ml-0.5 inline size-4 align-text-bottom" />
        </Link>
        <AccessBadge dataset={dataset} />
      </div>
      {dataset.description ? <p className="line-clamp-2 text-caption text-fg-muted">{dataset.description}</p> : null}
      <p className="flex items-center gap-1 text-caption text-fg-subtle">
        <Icon name="spaces" className="size-3.5" />
        {dataset.project}
      </p>
    </li>
  );
}
