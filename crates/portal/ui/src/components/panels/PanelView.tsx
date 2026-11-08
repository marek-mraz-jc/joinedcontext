/**
 * One dashboard panel in its card: its title and description, then by its state a skeleton,
 * the reason it has no data (a source the person may not read, a datasource that refused, an
 * empty range), or the chart its `type` names. A type this page does not draw shows its frames as
 * a table, the way the server-side page of daiya does.
 */
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button, Card, EmptyState, Icon, InlineError, Skeleton } from "../ui";
import type { Frame } from "./frames";
import { hasData } from "./frames";
import type { FieldConfigSource } from "./fieldConfig";
import { BarChartPanel, HeatmapPanel, HistogramPanel, PieChartPanel, StateTimelinePanel } from "./ChartPanels";
import { TablePanel } from "./TablePanel";
import { TimeSeriesPanel } from "./TimeSeriesPanel";
import { BarGaugePanel, GaugePanel, StatPanel } from "./ValuePanels";

/** A panel as the Portal's dashboard route answers it (queries stay on the server). */
export interface PanelDefinition {
  id: number;
  type: string;
  title?: string;
  description?: string;
  gridPos?: { x?: number; y?: number; w?: number; h?: number };
  fieldConfig?: FieldConfigSource;
  options?: Record<string, unknown>;
  collapsed?: boolean;
  panels?: PanelDefinition[];
  maxDataPoints?: number;
}

/** A panel's data as the Portal's panel route answers it. */
export interface PanelData {
  frames: Frame[];
  accessDenied?: string;
  error?: string;
}

/** The panel types drawn here; others fall back to a table. */
export const DRAWN = ["timeseries", "table", "stat", "gauge", "bargauge", "barchart", "piechart", "heatmap", "histogram", "state-timeline", "text"] as const;

export interface PanelViewProps {
  panel: PanelDefinition;
  data?: PanelData;
  loading?: boolean;
  /** The read itself failed (the Portal or the network), not the panel's datasource. */
  failure?: string;
  onRetry?: () => void;
  from: number;
  to: number;
  locale: string;
  /** The height the panel's body gets, from its `gridPos.h`. */
  height: number;
  headingLevel?: "h2" | "h3";
}

function Body({ panel, data, from, to, locale, height }: Required<Pick<PanelViewProps, "panel" | "from" | "to" | "locale" | "height">> & { data: PanelData }): ReactNode {
  const title = panel.title ?? "";
  const common = { title, frames: data.frames, fieldConfig: panel.fieldConfig, options: panel.options, height, locale };
  switch (panel.type) {
    case "timeseries":
      return <TimeSeriesPanel {...common} from={from} to={to} />;
    case "stat":
      return <StatPanel {...common} />;
    case "gauge":
      return <GaugePanel {...common} />;
    case "bargauge":
      return <BarGaugePanel {...common} />;
    case "barchart":
      return <BarChartPanel {...common} from={from} to={to} />;
    case "piechart":
      return <PieChartPanel {...common} from={from} to={to} />;
    case "heatmap":
      return <HeatmapPanel {...common} from={from} to={to} />;
    case "histogram":
      return <HistogramPanel {...common} from={from} to={to} />;
    case "state-timeline":
      return <StateTimelinePanel {...common} from={from} to={to} />;
    default:
      return <TablePanel title={title} frames={data.frames} fieldConfig={panel.fieldConfig} locale={locale} />;
  }
}

/** What the service said, folded: the operator's detail, not the person's next step. */
function Reason({ text }: { text: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <details className="max-w-full text-caption text-fg-muted">
      <summary className="focus-ring cursor-pointer rounded-sm">{t("dashboardView.details")}</summary>
      <p className="mt-1 [overflow-wrap:anywhere]">{text}</p>
    </details>
  );
}

export function PanelView({ panel, data, loading, failure, onRetry, from, to, locale, height, headingLevel = "h2" }: PanelViewProps): JSX.Element {
  const { t } = useTranslation();
  const Heading = headingLevel;
  const title = panel.title || t("dashboardView.untitled");
  const noValue = typeof panel.fieldConfig?.defaults?.noValue === "string" ? panel.fieldConfig.defaults.noValue : undefined;
  let body: ReactNode;
  if (panel.type === "text") {
    const content = typeof panel.options?.content === "string" ? panel.options.content : "";
    body = <p className="whitespace-pre-line text-body text-fg">{content.replace(/[#*_`>]/g, "").trim()}</p>;
  } else if (failure) {
    body = (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
        <InlineError>{t("dashboardView.failed")}</InlineError>
        <Reason text={failure} />
        {onRetry ? (
          <Button size="sm" variant="secondary" onClick={onRetry}>
            {t("dashboardView.retry")}
          </Button>
        ) : null}
      </div>
    );
  } else if (loading || !data) {
    body = (
      <div role="status" aria-label={t("dashboardView.loading", { title })}>
        <Skeleton className="h-full min-h-24 w-full" />
      </div>
    );
  } else if (data.accessDenied) {
    body = <EmptyState bare icon="lock" title={t("dashboardView.deniedTitle")} description={`${data.accessDenied} ${t("dashboardView.askOwner")}`} />;
  } else if (data.error) {
    body = (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
        <InlineError>{t("dashboardView.sourceFailed")}</InlineError>
        <Reason text={data.error} />
        {onRetry ? (
          <Button size="sm" variant="secondary" onClick={onRetry}>
            {t("dashboardView.retry")}
          </Button>
        ) : null}
      </div>
    );
  } else if (!hasData(data.frames)) {
    body = <EmptyState bare icon="dashboards" title={noValue ?? t("dashboardView.emptyTitle")} description={t("dashboardView.empty")} />;
  } else {
    body = <Body panel={panel} data={data} from={from} to={to} locale={locale} height={height} />;
  }
  return (
    <Card as="section" padding="sm" aria-label={title} className="flex h-full min-w-0 flex-col gap-2" data-panel-type={panel.type}>
      <div className="flex min-w-0 items-start gap-1">
        <Heading className="min-w-0 flex-1 truncate text-body font-semibold text-fg" title={title}>
          {title}
        </Heading>
        {!(DRAWN as readonly string[]).includes(panel.type) ? (
          <span className="shrink-0 text-caption text-fg-muted">{t("dashboardView.asTable", { type: panel.type })}</span>
        ) : null}
      </div>
      {panel.description ? (
        <p className="flex items-start gap-1 text-caption text-fg-muted">
          <Icon name="info" className="mt-0.5 size-3.5 shrink-0" />
          <span>{panel.description}</span>
        </p>
      ) : null}
      <div className="min-h-0 min-w-0 flex-1">{body}</div>
    </Card>
  );
}
