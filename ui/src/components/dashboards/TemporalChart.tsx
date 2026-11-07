import { useMemo } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { MAX_POINTS, unitTitle } from "@joinedcontext/sdk";
import { Button, Card, Icon, InlineError, Skeleton } from "../ui";
import { readerTimeZone, unitCodeOf } from "./typeCharts";

/** One reading: when it was observed, and what was read. */
interface Point {
  at: number;
  value: number;
}

/**
 * The instances of one property of one entity, in either shape the temporal representation
 * uses: `options=temporalValues` pairs (`[value, time]`), or plain instances carrying
 * `observedAt`. Anything that is not a number is left out — a chart of strings is not a chart.
 */
export function pointsOf(body: unknown, property: string): Point[] {
  const attribute = (body as Record<string, unknown> | null)?.[property];
  const instances = Array.isArray(attribute)
    ? attribute
    : ((attribute as { values?: unknown })?.values ?? []);
  const points: Point[] = [];
  for (const instance of Array.isArray(instances) ? instances : []) {
    const pair = Array.isArray(instance)
      ? { value: instance[0], time: instance[1] }
      : {
          value: (instance as { value?: unknown })?.value,
          time:
            (instance as { observedAt?: unknown })?.observedAt ??
            (instance as { modifiedAt?: unknown })?.modifiedAt,
        };
    const at = Date.parse(String(pair.time));
    const value = Number(pair.value);
    if (!Number.isNaN(at) && Number.isFinite(value)) {
      points.push({ at, value });
    }
  }
  return points.sort((a, b) => a.at - b.at);
}

/** The polyline of the points inside a 100×40 box, so the SVG scales with its card. */
export function polylineOf(points: Point[]): string {
  if (points.length === 0) {
    return "";
  }
  const times = points.map((point) => point.at);
  const values = points.map((point) => point.value);
  const [minAt, maxAt] = [Math.min(...times), Math.max(...times)];
  const [minValue, maxValue] = [Math.min(...values), Math.max(...values)];
  const spanAt = maxAt - minAt || 1;
  const spanValue = maxValue - minValue || 1;
  return points
    .map((point) => {
      const x = ((point.at - minAt) / spanAt) * 100;
      const y = 40 - ((point.value - minValue) / spanValue) * 40;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
}

/**
 * One property of one entity over time (UI-18): the history the endpoint serves, drawn as a
 * line. No chart library — a dozen points and a polyline do not need one.
 */
export function TemporalChart({
  slug,
  entityId,
  property,
  title,
}: {
  slug: string;
  entityId: string;
  property: string;
  title: string;
}): JSX.Element {
  const { t } = useTranslation();
  const history = useQuery({
    queryKey: ["temporal", slug, entityId, property],
    retry: false,
    staleTime: 60_000,
    queryFn: async () => {
      // The instances whole, not `temporalValues`: each carries its unit, which the axis states.
      // The newest MAX_POINTS, as the entity's history reads them: without lastN the broker
      // answers its first page, the oldest readings, and the chart ends wherever that page did.
      const url = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/temporal/entities/${encodeURIComponent(entityId)}?attrs=${encodeURIComponent(property)}&lastN=${MAX_POINTS}`;
      const response = await globalThis.fetch(
        new Request(`${window.location.origin}${url}`, { headers: { Accept: "application/ld+json" } }),
      );
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      return (await response.json()) as unknown;
    },
  });

  const points = useMemo(
    () => (history.data ? pointsOf(history.data, property) : []),
    [history.data, property],
  );
  const line = polylineOf(points);
  const last = points.at(-1);
  const instances = (history.data as Record<string, unknown> | undefined)?.[property];
  const unit = Array.isArray(instances) ? unitCodeOf(instances.map((instance) => ({ [property]: instance })), property) : undefined;

  return (
    <Card as="figure" padding="sm" className="flex flex-col gap-2">
      <figcaption className="text-body font-medium">{title}</figcaption>
      {history.isPending ? (
        // The shape of the chart, so the card does not grow under the one beside it when the
        // readings land.
        <div role="status" aria-busy="true" aria-label={t("app.loading")}>
          <Skeleton className="h-32 w-full" />
        </div>
      ) : null}
      {/* A history that could not be read is not "there is no history" (T-1763): the two mean
          opposite things, and only one of them is worth pressing a button about. */}
      {history.isError ? (
        <InlineError className="flex flex-wrap items-center gap-2">
          {t("dashboards.widget.historyFailed", {
            reason: history.error instanceof Error ? history.error.message : t("app.error.generic"),
          })}
          <Button
            size="sm"
            variant="ghost"
            icon={<Icon name="refresh" className="size-3.5" />}
            onClick={() => {
              void history.refetch();
            }}
          >
            {t("app.error.retry")}
          </Button>
        </InlineError>
      ) : null}
      {!history.isPending && !history.isError && points.length === 0 ? (
        <p role="status" className="text-caption text-fg-muted">
          {t("dashboards.widget.noHistory")}
        </p>
      ) : null}
      {line ? (
        <>
          <svg
            viewBox="0 0 100 40"
            preserveAspectRatio="none"
            className="h-32 w-full"
            role="img"
            aria-label={t("dashboards.widget.chartLabel", { property, entity: entityId })}
          >
            <polyline
              points={line}
              fill="none"
              stroke="currentColor"
              strokeWidth="1"
              vectorEffect="non-scaling-stroke"
              className="text-primary-soft-fg"
            />
          </svg>
          <p className="text-caption text-fg-muted">
            {unit
              ? t("dashboards.widget.timeAxisUnit", { zone: readerTimeZone(), unit: unitTitle(unit) || unit })
              : t("dashboards.widget.timeAxis", { zone: readerTimeZone() })}
          </p>
          <p className="text-caption text-fg-muted">
            {t("dashboards.widget.latest", {
              property,
              value: String(last?.value ?? ""),
              count: points.length,
            })}
          </p>
        </>
      ) : null}
    </Card>
  );
}
