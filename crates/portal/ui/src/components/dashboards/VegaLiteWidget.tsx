/**
 * A `vega-lite` widget (T-6786): the author's Vega-Lite 5 spec over one source of the widget's
 * Endpoint, read with the reader's session (so the Endpoint's Policy decides every row) and
 * filled into the spec's dataset `source`. The spec was checked as data where it was written
 * (jc-core `dashboard.rs`); {@link VegaChart} draws it with the expression interpreter and a
 * loader that loads nothing, so a spec cannot fetch or run code here either.
 */
import { useQuery } from "@tanstack/react-query";
import type { JSX } from "react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Card, Skeleton } from "../ui";
import { PageFailed } from "../ui/PageState";
import { VegaChart, loadVega } from "../panels/VegaChart";
import type { Spec } from "../panels/specs";
import { readSource } from "./vegaLiteSource";
import type { WidgetSource } from "./vegaLiteSource";

/** The chart's height when its spec names none, and the plot's inside it (axes take the rest). */
const HEIGHT = 280;
const PLOT_HEIGHT = 220;

/** The spec's own title, when it gives one as text. */
export function specTitle(spec: Spec): string | null {
  const title = spec.title;
  if (typeof title === "string") return title;
  if (title && typeof title === "object" && typeof (title as { text?: unknown }).text === "string") return (title as { text: string }).text;
  return null;
}

/** The spec with the rows as its dataset `source`, sized to the card unless it says otherwise. */
export function withRows(spec: Spec, rows: Record<string, unknown>[]): Spec {
  const { title: _title, ...rest } = spec;
  return {
    width: "container",
    // A plot fills the card: without a height a band scale draws 20 px per row and leaves the rest empty.
    height: PLOT_HEIGHT,
    autosize: { type: "fit", contains: "padding" },
    ...rest,
    datasets: { ...((spec.datasets as Record<string, unknown>) ?? {}), source: rows },
  };
}

export function VegaLiteWidget({
  slug,
  source,
  spec,
  fallbackTitle,
}: {
  slug: string;
  source: WidgetSource;
  spec: Spec;
  fallbackTitle: string;
}): JSX.Element {
  const { t } = useTranslation();
  const title = specTitle(spec) ?? fallbackTitle;
  const read = useQuery({
    queryKey: ["vega-lite-widget", slug, source],
    retry: false,
    staleTime: 60_000,
    queryFn: async () => {
      const got = await readSource(slug, source);
      if (got.csv === undefined) return got;
      const [vega] = await loadVega();
      return { ...got, rows: vega.read(got.csv, { type: "csv", parse: "auto" }) as Record<string, unknown>[] };
    },
  });
  const drawn = useMemo(() => (read.data ? withRows(spec, read.data.rows) : null), [spec, read.data]);
  const height = typeof spec.height === "number" ? spec.height + HEIGHT - PLOT_HEIGHT : HEIGHT;

  return (
    <Card as="figure" padding="sm" className="flex flex-col gap-2">
      <figcaption className="text-body font-medium">{title}</figcaption>
      {read.isPending ? (
        <div role="status" aria-busy="true" aria-label={t("app.loading")}>
          <Skeleton className="h-32 w-full" />
        </div>
      ) : read.isError ? (
        <PageFailed error={read.error} onRetry={() => void read.refetch()} />
      ) : read.data.rows.length === 0 ? (
        <p role="status" className="text-caption text-fg-muted">
          {t("dashboards.widget.vegaNoRows")}
        </p>
      ) : (
        <>
          <VegaChart spec={drawn ?? spec} label={title} height={height} />
          {read.data.cut ? <p className="text-caption text-fg-muted">{t("dashboards.widget.vegaCut", { count: read.data.rows.length })}</p> : null}
        </>
      )}
    </Card>
  );
}
