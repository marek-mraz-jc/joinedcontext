/**
 * One ECharts chart in the organization's colours (AP-123, T-2921): the SDK's `echartsTheme`
 * carries the branding palette, the SVG renderer keeps the chart crisp at every width and
 * readable in a screenshot. What the chart says is also the `aria-label`, so a screen reader gets
 * the same fact the picture gives.
 */
import { useEffect, useRef } from "react";
import { init, registerTheme, use } from "echarts/core";
import type { EChartsCoreOption } from "echarts/core";
import { BarChart } from "echarts/charts";
import { GridComponent, LegendComponent, TooltipComponent } from "echarts/components";
import { SVGRenderer } from "echarts/renderers";
import { echartsTheme } from "@joinedcontext/sdk";

// Only what the screen draws: bars on a grid, a legend and a tooltip, in SVG.
use([BarChart, GridComponent, LegendComponent, TooltipComponent, SVGRenderer]);

export function Chart({ option, label, height = 260 }: { option: EChartsCoreOption; label: string; height?: number }) {
  const host = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const element = host.current;
    if (!element) return;
    registerTheme("jc", echartsTheme());
    const chart = init(element, "jc", { renderer: "svg" });
    chart.setOption(option);
    const resize = new ResizeObserver(() => chart.resize());
    resize.observe(element);
    return () => {
      resize.disconnect();
      chart.dispose();
    };
  }, [option]);

  return <div ref={host} className="chart" role="img" aria-label={label} style={{ height }} />;
}
