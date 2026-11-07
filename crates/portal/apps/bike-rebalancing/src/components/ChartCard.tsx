import type { ReactNode } from "react";
import { useEffect, useRef } from "react";
import * as echarts from "echarts/core";
import { BarChart, LineChart, ScatterChart } from "echarts/charts";
import { GridComponent, LegendComponent, MarkLineComponent, TooltipComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";

// Only the charts and parts the app draws: the whole library would triple the bundle.
echarts.use([BarChart, LineChart, ScatterChart, GridComponent, LegendComponent, MarkLineComponent, TooltipComponent, CanvasRenderer]);
import { currentTokens, echartsTheme, Empty, Loading } from "@joinedcontext/sdk";

/** One ECharts chart with a caption, in the SDK's theme; the loading and empty states in its place. */
export function ChartCard({
  title,
  option,
  height = 280,
  loading,
  empty,
  loadingLabel,
}: {
  title: string;
  option: Record<string, unknown> | null;
  height?: number;
  loading?: boolean;
  empty?: ReactNode;
  /** What the card says while the data is read, in the page's language. */
  loadingLabel?: string;
}): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const showCanvas = !loading && option !== null;

  useEffect(() => {
    const el = containerRef.current;
    if (!showCanvas || !el) return;
    const chart = echarts.init(el, echartsTheme(currentTokens()), { renderer: "canvas" });
    chartRef.current = chart;
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
