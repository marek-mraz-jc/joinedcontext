import type { ReactNode } from "react";
import { useEffect, useRef } from "react";
import * as echarts from "echarts/core";
import { BarChart, HeatmapChart, LineChart, ScatterChart } from "echarts/charts";
import { GridComponent, LegendComponent, MarkLineComponent, TooltipComponent, VisualMapComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";

// Only the charts and parts the app draws: the whole library would triple the bundle.
echarts.use([BarChart, HeatmapChart, LineChart, ScatterChart, GridComponent, LegendComponent, MarkLineComponent, TooltipComponent, VisualMapComponent, CanvasRenderer]);
import { currentTokens, echartsTheme, Empty, Loading } from "@joinedcontext/sdk";

/** One ECharts chart with a caption, in the SDK's theme; the loading, error and empty states in its place. */
export function ChartCard({
  title,
  option,
  height = 280,
  loading,
  empty,
  loadingLabel,
  onSelect,
}: {
  title: string;
  option: Record<string, unknown> | null;
  height?: number;
  loading?: boolean;
  empty?: ReactNode;
  /** What the card says while the data is read, in the page's language. */
  loadingLabel?: string;
  /** Called with the category of the bar a person clicks. */
  onSelect?: (name: string) => void;
}): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const showCanvas = !loading && option !== null;
  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;

  useEffect(() => {
    const el = containerRef.current;
    if (!showCanvas || !el) return;
    const chart = echarts.init(el, echartsTheme(currentTokens()), { renderer: "canvas" });
    chartRef.current = chart;
    chart.on("click", (params: unknown) => {
      const name = (params as { name?: unknown }).name;
      if (name !== undefined) onSelectRef.current?.(String(name));
    });
    const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(() => chart.resize());
    observer?.observe(el);
    return () => {
      observer?.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, [showCanvas]);

  useEffect(() => {
    if (chartRef.current && option) chartRef.current.setOption(option, true);
  }, [option, showCanvas]);

  return (
    <figure className="jc-chart">
      <figcaption>{title}</figcaption>
      {loading ? (
        <Loading label={loadingLabel} />
      ) : option === null ? (
        <Empty>{empty}</Empty>
      ) : (
        <div className="jc-chart-canvas" role="img" aria-label={title} style={{ height }} ref={containerRef} />
      )}
    </figure>
  );
}
