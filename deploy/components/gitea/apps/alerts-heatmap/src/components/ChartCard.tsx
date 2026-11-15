import type { ReactNode } from "react";
import { useEffect, useRef } from "react";
import * as echarts from "echarts";
import { currentTokens, echartsTheme, Empty, Loading } from "@joinedcontext/sdk";

/** One ECharts chart with a caption, in the SDK's theme; the loading and empty states in its place. */
export function ChartCard({
  title,
  option,
  height = 280,
  loading,
  empty,
  onPick,
}: {
  title: string;
  option: Record<string, unknown> | null;
  height?: number;
  loading?: boolean;
  empty?: ReactNode;
  /** Called with ECharts' own description of what was clicked: a heat map cell's value. */
  onPick?: (params: unknown) => void;
}): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const showCanvas = !loading && option !== null;
  const onPickRef = useRef(onPick);
  onPickRef.current = onPick;

  useEffect(() => {
    const el = containerRef.current;
    if (!showCanvas || !el) return;
    const chart = echarts.init(el, echartsTheme(currentTokens()), { renderer: "canvas" });
    chartRef.current = chart;
    chart.on("click", (params: unknown) => onPickRef.current?.(params));
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
        <Loading />
      ) : option === null ? (
        <Empty>{empty}</Empty>
      ) : (
        <div className="jc-chart-canvas" role="img" aria-label={title} style={{ height }} ref={containerRef} />
      )}
    </figure>
  );
}
