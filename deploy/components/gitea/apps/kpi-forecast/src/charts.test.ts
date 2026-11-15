import { describe, expect, it } from "vitest";
import type { SeriesResult } from "./analysis";
import { detailOption, tooltipOf } from "./charts";

const result = (over: Partial<SeriesResult>): SeriesResult => ({
  id: "a",
  status: "ok",
  n: 3,
  first: 0,
  last: 2,
  latest: 3,
  trend: null,
  step: 1,
  season: 0,
  weights: null,
  sigma: 0.5,
  history: [
    [0, 1],
    [1, 2],
    [2, 3],
  ],
  fitted: [[1, 2]],
  forecast: [{ t: 3, v: 4, lo: 3, hi: 5 }],
  anomalies: [{ t: 1, v: 2, expected: 1.5 }],
  ...over,
});

describe("the indicator's chart", () => {
  it("draws history, model, forecast, its band from floor to ceiling, and the odd points", () => {
    const option = detailOption(result({}), "en") as { series: { name: string; data: unknown[] }[] };
    const byName = (name: string) => option.series.filter((s) => s.name === name);
    expect(byName("Measured")[0].data).toHaveLength(3);
    expect(byName("Forecast")[0].data).toEqual([[3, 4]]);
    expect(byName("95 % interval").map((s) => s.data)).toEqual([[[3, 3]], [[3, 2]]]);
    expect(byName("Looks wrong")[0].data).toHaveLength(1);
  });

  it("is nothing for an indicator with no history", () => {
    expect(detailOption(result({ history: [], status: "empty" }), "fi")).toBeNull();
  });

  it("tells the moment on Helsinki's clock and each line's value, the band left out", () => {
    const at = Date.UTC(2030, 9, 3, 9, 0);
    const text = tooltipOf(
      [
        { seriesName: "Measured", marker: "●", value: [at, 3848] },
        { seriesName: "95 % interval", value: [at, 10] },
        { seriesName: "Model", marker: "○", value: [at, 3799.5] },
      ],
      "en",
    );
    expect(text).toBe("3 Oct 2030, 12:00<br/>●Measured: 3,848<br/>○Model: 3,800");
    expect(tooltipOf([], "fi")).toBe("");
  });

  it("writes its axes and tooltip in the language and Helsinki's time", () => {
    const option = detailOption(result({}), "fi") as {
      tooltip: { formatter: (p: unknown) => string };
      xAxis: { axisLabel: { formatter: (ms: number) => string } };
      yAxis: { axisLabel: { formatter: (v: number) => string } };
    };
    const at = Date.UTC(2030, 9, 3, 9, 0);
    expect(option.xAxis.axisLabel.formatter(at)).toBe("3.10.");
    expect(option.yAxis.axisLabel.formatter(3848)).toBe("3\u00a0848");
    expect(option.tooltip.formatter([{ seriesName: "Mitattu", marker: "", value: [at, 5.5] }])).toBe("3.10.2030 klo 12.00<br/>Mitattu: 5,5");
  });
});
