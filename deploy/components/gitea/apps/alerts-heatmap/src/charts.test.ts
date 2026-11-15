import { describe, expect, it } from "vitest";
import { cellOf, weekOption } from "./charts";

const empty = () => Array.from({ length: 7 }, () => Array.from({ length: 24 }, () => 0));

describe("the hours of the week", () => {
  it("draws Monday at the top and says nothing when no alert has a time", () => {
    expect(weekOption(empty(), "fi")).toBeNull();
    const matrix = empty();
    matrix[0][8] = 4;
    matrix[6][23] = 1;
    const option = weekOption(matrix, "fi") as { yAxis: { data: string[] }; series: { data: number[][] }[]; visualMap: { max: number } };
    expect(option.yAxis.data).toEqual(["su", "la", "pe", "to", "ke", "ti", "ma"]);
    expect(option.series[0].data).toContainEqual([8, 6, 4]);
    expect(option.series[0].data).toContainEqual([23, 0, 1]);
    expect(option.visualMap.max).toBe(4);
  });

  it("reads a clicked cell back as a weekday and an hour", () => {
    expect(cellOf({ value: [8, 6, 4] })).toEqual({ weekday: 0, hour: 8 });
    expect(cellOf({ value: [23, 0, 1] })).toEqual({ weekday: 6, hour: 23 });
    expect(cellOf({ name: "Mon" })).toBeNull();
  });
});
