/**
 * A `bar-chart` or `histogram` widget (UI-93, T-3257): one attribute over an entity type, read
 * through the widget's Endpoint with its filter and the reader's session, so a dashboard never
 * shows more than the grant permits. Drawn as SVG with the numbers beside the bars, the unit on
 * the value axis, and a list of the same numbers for a screen reader.
 */
import { useQuery } from "@tanstack/react-query";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { unitTitle } from "@joinedcontext/sdk";
import { fetchEntities } from "../entities/filters";
import { Card, Skeleton } from "../ui";
import { PageFailed } from "../ui/PageState";
import { collect } from "../../pages/explore/exportView";
import { barsOf, binsOf, unitCodeOf } from "./typeCharts";

/** The most entities a chart over a type reads; past it, the chart says it counted these. */
export const CHART_MAX = 5_000;

export function TypeChart({
  slug,
  entityType,
  property,
  q,
  kind,
  title,
}: {
  slug: string;
  entityType: string;
  property: string;
  q?: string;
  kind: "bar-chart" | "histogram";
  title: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const read = useQuery({
    queryKey: ["type-chart", slug, entityType, property, q],
    retry: false,
    staleTime: 60_000,
    queryFn: () =>
      collect(
        async (offset, limit) => {
          // Normalized, so the attribute's unit comes with its values.
          const page = await fetchEntities(slug, { type: entityType, attrs: [property], q }, { limit, offset, keyValues: false, count: offset === 0 });
          return { rows: page.rows as Record<string, unknown>[], count: page.count };
        },
        { max: CHART_MAX },
      ),
  });
  const number = new Intl.NumberFormat(i18n.language, { maximumFractionDigits: 2 });

  if (read.isPending) {
    return (
      <Card as="figure" padding="sm" className="flex flex-col gap-2">
        <figcaption className="text-body font-medium">{title}</figcaption>
        <div role="status" aria-busy="true" aria-label={t("app.loading")}>
          <Skeleton className="h-32 w-full" />
        </div>
      </Card>
    );
  }
  if (read.isError) {
    return (
      <Card as="figure" padding="sm" className="flex flex-col gap-2">
        <figcaption className="text-body font-medium">{title}</figcaption>
        <PageFailed error={read.error} onRetry={() => void read.refetch()} />
      </Card>
    );
  }

  const entities = read.data.entities;
  const unit = unitCodeOf(entities, property);
  const unitWords = unit ? unitTitle(unit) || unit : undefined;
  const rows =
    kind === "bar-chart"
      ? (() => {
          const { bars, rest } = barsOf(entities, property);
          return [...bars, ...(rest > 0 ? [{ label: t("dashboards.widget.rest"), count: rest }] : [])];
        })()
      : binsOf(entities, property).map((bin) => ({ label: `${number.format(bin.from)} – ${number.format(bin.to)}`, count: bin.count }));
  const most = Math.max(1, ...rows.map((row) => row.count));
  const height = rows.length * 10;

  return (
    <Card as="figure" padding="sm" className="flex flex-col gap-2">
      <figcaption className="text-body font-medium">{title}</figcaption>
      {rows.length === 0 ? (
        <p role="status" className="text-caption text-fg-muted">
          {t("dashboards.widget.noValues", { property })}
        </p>
      ) : (
        <>
          <svg viewBox={`0 0 300 ${height}`} className="w-full" role="img" aria-label={title}>
            {rows.map((row, index) => (
              <g key={row.label}>
                <rect
                  x="0"
                  y={index * 10 + 1}
                  width={Math.max(1, (row.count / most) * 180)}
                  height="8"
                  className="fill-primary-soft-fg"
                />
                <text x={(row.count / most) * 180 + 4} y={index * 10 + 7.5} fontSize="8" className="fill-fg">
                  {`${row.label}: ${number.format(row.count)}`}
                </text>
              </g>
            ))}
          </svg>
          <p className="text-caption text-fg-muted">
            {kind === "bar-chart"
              ? t("dashboards.widget.barAxis", { property })
              : unitWords
                ? t("dashboards.widget.histogramAxisUnit", { property, unit: unitWords })
                : t("dashboards.widget.histogramAxis", { property })}
            {read.data.truncated ? ` ${t("dashboards.widget.counted", { count: entities.length })}` : ""}
          </p>
          <ul className="sr-only" aria-label={kind === "bar-chart" ? property : unitWords ? `${property} (${unitWords})` : property}>
            {rows.map((row) => (
              <li key={row.label}>{t("dashboards.widget.row", { label: row.label, count: row.count })}</li>
            ))}
          </ul>
        </>
      )}
    </Card>
  );
}
