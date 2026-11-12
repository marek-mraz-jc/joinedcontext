import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Card } from "../ui";
import { TypeLink } from "../../pages/models/ModelLinks";
import { GridWidget } from "./GridWidget";
import { TemporalChart } from "./TemporalChart";
import { TypeChart } from "./TypeChart";
import { VegaLiteWidget } from "./VegaLiteWidget";
import type { WidgetSource } from "./vegaLiteSource";

/** A widget of an analytics page (UI-18), as jc-core's `Widget` writes it. */
export interface WidgetSpec {
  widgetType: string;
  endpointRef?: string;
  entityId?: string;
  property?: string;
  /**
   * The entity type a `grid` shows (T-1440), or a `bar-chart` or `histogram` counts (UI-93); the
   * grid's own configuration; the filter a chart over a type reads with.
   */
  entityType?: string;
  grid?: Record<string, unknown>;
  q?: string;
  /** A `vega-lite` widget's source of its Endpoint and its spec (T-6786). */
  data?: WidgetSource;
  spec?: Record<string, unknown>;
}

/**
 * The widgets of one dashboard page, each reading through its Endpoint's slug: the dashboards page
 * and the JSON editor's preview draw the same thing (T-6791).
 */
export function DashboardWidgets({
  project,
  widgets,
}: {
  project: string;
  widgets: { widget: WidgetSpec; slug?: string }[];
}): JSX.Element | null {
  const { t } = useTranslation();
  if (widgets.length === 0) {
    return null;
  }
  return (
    <div aria-label={t("dashboards.widgets")} className={widgets.length === 1 ? "grid gap-3" : "grid gap-3 md:grid-cols-2"}>
      {widgets.map(({ widget, slug }, index) =>
        widget.widgetType === "grid" && slug && widget.entityType ? (
          // The same grid the explorer renders, configured by the manifest (UI-71, SDK-30):
          // the endpoint and the type come from the widget, so a dashboard cannot point it
          // anywhere else, and the endpoint's Policy still decides every read and write.
          <GridWidget
            key={`${widget.endpointRef}-${widget.entityType}-${index}`}
            project={project}
            slug={slug}
            type={widget.entityType}
            config={widget.grid}
            title={<TypeLink project={project} type={widget.entityType} />}
          />
        ) : widget.widgetType === "temporal-chart" && slug && widget.entityId && widget.property ? (
          <TemporalChart
            key={`${widget.entityId}-${widget.property}-${index}`}
            slug={slug}
            entityId={widget.entityId}
            property={widget.property}
            title={t("dashboards.widget.temporalChart", { property: widget.property })}
          />
        ) : (widget.widgetType === "bar-chart" || widget.widgetType === "histogram") && slug && widget.entityType && widget.property ? (
          <TypeChart
            key={`${widget.widgetType}-${widget.entityType}-${widget.property}-${index}`}
            slug={slug}
            entityType={widget.entityType}
            property={widget.property}
            q={widget.q}
            kind={widget.widgetType}
            title={t(`dashboards.widget.${widget.widgetType === "bar-chart" ? "barTitle" : "histogramTitle"}`, {
              property: widget.property,
              type: widget.entityType,
            })}
          />
        ) : widget.widgetType === "vega-lite" && slug && widget.data && widget.spec ? (
          <VegaLiteWidget
            key={`vega-lite-${widget.endpointRef}-${index}`}
            slug={slug}
            source={widget.data}
            spec={widget.spec}
            fallbackTitle={t("dashboards.widget.vegaTitle", {
              type: "type" in widget.data ? widget.data.type : "collection" in widget.data ? widget.data.collection : widget.data.entitySet,
            })}
          />
        ) : (
          <Card padding="sm" key={`${widget.widgetType}-${index}`}>
            <p role="status" className="text-body text-fg-muted">
              {t("dashboards.widget.unsupported", { type: widget.widgetType })}
            </p>
          </Card>
        ),
      )}
    </div>
  );
}
