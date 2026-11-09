/**
 * One dashboard panel in its card: its title and description, then by its state a skeleton,
 * the reason it has no data (a source the person may not read, a datasource that refused, an
 * empty range), or the chart its `type` names. A type this page does not draw shows its frames as
 * a table, the way the server-side page of daiya does.
 */
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { Button, buttonClass, Card, Icon, InlineError, safeHref, Skeleton } from "../ui";
import type { IconName } from "../ui";
import type { Frame } from "./frames";
import { hasData, outsideRange } from "./frames";
import type { FieldConfigSource } from "./fieldConfig";
import { BarChartPanel, HeatmapPanel, HistogramPanel, PieChartPanel, StateTimelinePanel } from "./ChartPanels";
import { GeomapPanel, MapFrame } from "./GeomapPanel";
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
  /** The panel's own range (`1h`), read only under a relative dashboard range, as Grafana does. */
  timeFrom?: string;
  /** How far back the panel's range moves (`1d`). */
  timeShift?: string;
  /** Hides the words of the panel's own range from its header. */
  hideTimeOverride?: boolean;
}

/** A panel's data as the Portal's panel route answers it. */
export interface PanelData {
  frames: Frame[];
  accessDenied?: string;
  error?: string;
}

/** The panel types drawn here; others fall back to a table. */
export const DRAWN = ["timeseries", "table", "stat", "gauge", "bargauge", "barchart", "piechart", "heatmap", "histogram", "state-timeline", "text", "geomap"] as const;

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
  /** The range and the picked variables, in words, for an empty panel to name. */
  asked?: { range: string; vars?: string };
  /** Ask a longer range: an empty panel's next step. */
  onWiden?: () => void;
  /** Move the dashboard to the span the data covers: Grafana's "Zoom to data". */
  onZoom?: (from: number, to: number) => void;
  /** The page already says several panels failed: a failed panel shows its title line only. */
  quiet?: boolean;
  /** The panel's own range in Grafana's words (`Last 1 hour + timeshift -1d`), beside its title. */
  timeInfo?: string;
  /** The project, whose basemap route a geomap panel draws on. */
  project?: string;
  /** The dashboard as tables (daiya's own page): where a panel that cannot draw its rows sends a person. */
  tablesHref?: string;
}

function Body({ panel, data, from, to, locale, height, project, tablesHref }: Required<Pick<PanelViewProps, "panel" | "from" | "to" | "locale" | "height">> & { data: PanelData; project?: string; tablesHref?: string }): ReactNode {
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
    case "geomap":
      return (
        <GeomapPanel
          {...common}
          project={project}
          empty={(text, description) => (
            <MapFrame height={height}>
              <PanelState icon="globe" title={text} description={description}>
                {tablesHref ? <TablesLink href={tablesHref} /> : null}
              </PanelState>
            </MapFrame>
          )}
        />
      );
    default:
      return <TablePanel title={title} frames={data.frames} fieldConfig={panel.fieldConfig} options={panel.options} locale={locale} />;
  }
}

/**
 * A text panel's Markdown as the plain text the Portal shows: links and images by their words,
 * no tags, heading or quote marks, emphasis or code ticks; list items keep a bullet.
 */
export function plainText(markdown: string): string {
  return markdown
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/<[^>]*>/g, "")
    .replace(/^ {0,3}(#{1,6}|>)\s*/gm, "")
    .replace(/^ {0,3}[-*+]\s+/gm, "• ")
    .replace(/[*_`~]/g, "")
    .trim();
}

/** The dashboard's table page, opened beside this one. */
function TablesLink({ href }: { href: string }): JSX.Element | null {
  const { t } = useTranslation();
  const safe = safeHref(href);
  if (!safe) return null;
  return (
    <a className={clsx(buttonClass("secondary", "sm"), "touch-target")} data-tables-link href={safe} target="_blank" rel="noopener noreferrer">
      <Icon name="external" className="size-4" />
      {t("dashboardView.asTables")}
    </a>
  );
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

/**
 * A panel's state in place of its chart, centred in the panel: an icon badge, a title, what to do
 * next and the action. A failure is an alert in the danger tone; the others are a status.
 */
export function PanelState({ danger, icon, title, description, children }: { danger?: boolean; icon: IconName; title: string; description?: string; children?: ReactNode }): JSX.Element {
  return (
    <div
      role={danger ? "alert" : "status"}
      data-empty-state={danger ? undefined : ""}
      className="flex h-full min-h-24 flex-col items-center justify-center gap-1.5 px-2 py-3 text-center"
    >
      <span className={clsx("inline-flex size-9 items-center justify-center rounded-full", danger ? "bg-danger-soft text-danger" : "bg-primary-soft text-primary-soft-fg")}>
        <Icon name={icon} className="size-4" />
      </span>
      <p className="text-body font-semibold text-fg">{title}</p>
      {description ? <p className="max-w-md text-caption text-fg-muted">{description}</p> : null}
      {children ? <div className="mt-1 flex flex-col items-center gap-2">{children}</div> : null}
    </div>
  );
}

/** A sentence ends in a stop before the next one is appended. */
function sentence(text: string): string {
  const trimmed = text.trim();
  return /[.!?…]$/.test(trimmed) ? trimmed : `${trimmed}.`;
}

export function PanelView({ panel, data, loading, failure, onRetry, from, to, locale, height, headingLevel = "h2", asked, onWiden, onZoom, quiet, timeInfo, project, tablesHref }: PanelViewProps): JSX.Element {
  const { t } = useTranslation();
  const Heading = headingLevel;
  const title = panel.title || t("dashboardView.untitled");
  const noValue = typeof panel.fieldConfig?.defaults?.noValue === "string" ? panel.fieldConfig.defaults.noValue : undefined;
  let body: ReactNode;
  if (panel.type === "text") {
    const content = typeof panel.options?.content === "string" ? panel.options.content : "";
    body = <p className="whitespace-pre-line text-body text-fg [overflow-wrap:anywhere]">{plainText(content)}</p>;
  } else if (failure || data?.error) {
    const reason = failure ?? data?.error ?? "";
    body = quiet ? (
      <div className="flex h-full min-h-12 items-center justify-center">
        <InlineError>{t("dashboardView.notRead")}</InlineError>
      </div>
    ) : (
      <PanelState danger icon="error" title={t(failure ? "dashboardView.failedTitle" : "dashboardView.sourceFailedTitle")} description={t(failure ? "dashboardView.failed" : "dashboardView.sourceFailed")}>
        {onRetry ? (
          <Button size="sm" variant="secondary" onClick={onRetry}>
            <Icon name="refresh" className="size-4" />
            {t("dashboardView.retry")}
          </Button>
        ) : null}
        <Reason text={reason} />
      </PanelState>
    );
  } else if (loading || !data) {
    body = (
      <div role="status" aria-label={t("dashboardView.loading", { title })}>
        <Skeleton className="h-full min-h-24 w-full" />
      </div>
    );
  } else if (data.accessDenied) {
    body = <PanelState icon="lock" title={t("dashboardView.deniedTitle")} description={`${sentence(data.accessDenied)} ${t("dashboardView.askOwner")}`} />;
  } else if (!hasData(data.frames) && panel.type === "geomap") {
    // A map reads each entity's latest values: a longer range would bring nothing more.
    body = (
      <MapFrame height={height}>
        <PanelState icon="globe" title={noValue ?? t("dashboardView.emptyTitle")} description={t("dashboardView.geomap.empty")}>
          {tablesHref ? <TablesLink href={tablesHref} /> : null}
        </PanelState>
      </MapFrame>
    );
  } else if (!hasData(data.frames)) {
    body = (
      <PanelState icon="dashboards" title={noValue ?? t("dashboardView.emptyTitle")} description={asked ? t(asked.vars ? "dashboardView.emptyInFor" : "dashboardView.emptyIn", asked) : t("dashboardView.empty")}>
        {onWiden ? (
          <Button size="sm" variant="secondary" onClick={onWiden}>
            {t("dashboardView.widen")}
          </Button>
        ) : null}
      </PanelState>
    );
  } else if (panel.type === "timeseries" && outsideRange(data.frames, from, to)) {
    const span = outsideRange(data.frames, from, to)!;
    body = (
      <PanelState icon="clock" title={t("dashboardView.outsideTitle")} description={t("dashboardView.outside")}>
        {onZoom ? (
          <Button size="sm" variant="secondary" onClick={() => onZoom(span.from, span.to)}>
            {t("dashboardView.zoomToData")}
          </Button>
        ) : null}
      </PanelState>
    );
  } else {
    body = <Body panel={panel} data={data} from={from} to={to} locale={locale} height={height} project={project} tablesHref={tablesHref} />;
  }
  return (
    <Card as="section" padding="sm" aria-label={title} className="flex h-full min-w-0 flex-col gap-2" data-panel-type={panel.type}>
      <div className="flex min-w-0 items-start gap-1">
        <Heading className="min-w-0 flex-1 truncate text-body font-semibold text-fg" title={title}>
          {title}
        </Heading>
        {timeInfo ? (
          <span className="inline-flex shrink-0 items-center gap-1 text-caption text-primary-soft-fg" data-time-info="">
            <Icon name="clock" className="size-3.5" />
            {timeInfo}
          </span>
        ) : null}
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
