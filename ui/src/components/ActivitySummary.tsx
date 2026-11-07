import type { JSX } from "react";
import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../api/client";
import { activitySearch } from "../api/activity";
import type { ActivityEvent } from "../api/activity";
import { Card } from "./ui";

/** The five numbers the strip shows, in the order it shows them (UI-31). */
export const BUCKETS = [
  { key: "messages", kinds: ["pipeline.throughput"] },
  { key: "changes", kinds: ["config.applied", "change.merged"] },
  { key: "requests", kinds: ["endpoint.traffic"] },
  { key: "forwards", kinds: ["federation.forward"] },
  { key: "denials", kinds: ["access.denied"] },
] as const;

export type Bucket = (typeof BUCKETS)[number]["key"];

/**
 * What happened, as one number per bucket. An event that counted something carries the number in
 * `details.count`; one that reports a single thing happening counts as one.
 */
export function summarise(events: ActivityEvent[]): Record<Bucket, number> {
  const totals = Object.fromEntries(BUCKETS.map((b) => [b.key, 0])) as Record<Bucket, number>;
  for (const event of events) {
    const bucket = BUCKETS.find((b) => (b.kinds as readonly string[]).includes(event.kind));
    if (!bucket) {
      continue;
    }
    const counted = (event.details as { count?: unknown } | undefined)?.count;
    totals[bucket.key] += typeof counted === "number" && counted >= 0 ? counted : 1;
  }
  return totals;
}

const WINDOW_MS = 60 * 60 * 1000;

/** "What happened in the last hour", over the same events the feed below it lists. */
export function ActivitySummary({ project }: { project: string }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  // The hour before the page opened, read once: a window that moved on every render would
  // make a new query key on every render too.
  const [since] = useState(() => new Date(Date.now() - WINDOW_MS).toISOString());
  const query = { since, limit: 200 };

  const page = useQuery({
    queryKey: ["projects", project, "activity", "summary", since],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/activity", {
          params: { path: { project }, query: activitySearch(query) },
        }),
      ),
  });

  const totals = useMemo(
    () => summarise((page.data?.items ?? []) as ActivityEvent[]),
    [page.data],
  );

  if (page.isError) {
    return null;
  }

  return (
    <Card>
      <h2 className="text-caption font-medium text-fg-muted">{t("activity.lastHour")}</h2>
      <dl className="mt-3 grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-5">
        {BUCKETS.map((bucket) => (
          <div key={bucket.key} className="flex flex-col gap-0.5">
            <dt className="text-caption text-fg-muted">{t(`activity.bucket.${bucket.key}`)}</dt>
            <dd className="text-title font-semibold tabular-nums text-fg">
              {page.isPending ? "—" : totals[bucket.key].toLocaleString(locale)}
            </dd>
          </div>
        ))}
      </dl>
    </Card>
  );
}
