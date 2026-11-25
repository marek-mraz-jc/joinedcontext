// T-7706: frames and reducers edges: a boolean reading, a frame with no time, a gap among the readings, no reducer asked, a boolean calc value.
import { describe, expect, it } from "vitest";
import { numberOf, outsideRange, seriesOf } from "../src/components/panels/frames";
import type { Frame } from "../src/components/panels/frames";
import { calcValueOf, reduceField } from "../src/components/panels/reducers";

describe("frames", () => {
  it("reads a boolean as 1 or 0", () => {
    expect(numberOf(true)).toBe(1);
    expect(numberOf(false)).toBe(0);
  });

  it("draws no series for a frame without a time field", () => {
    const frame: Frame = { fields: [{ name: "pm10", type: "number", values: [1, 2] }] };
    expect(seriesOf([frame])).toEqual([]);
  });

  it("skips a gap when it looks for readings outside the range", () => {
    const frame: Frame = {
      fields: [
        { name: "t", type: "time", values: [1000, 2000, 3000] },
        { name: "pm10", type: "number", values: [null, 4, 5] },
      ],
    };
    expect(outsideRange([frame], 10_000, 20_000)).toEqual({ from: 2000, to: 3000 });
    expect(outsideRange([frame], 0, 2500)).toBeUndefined();
  });
});

describe("reducers", () => {
  it("answers nothing when no known reducer is asked", () => {
    expect(reduceField({ values: [1, 2], numeric: true }, [])).toEqual({});
    expect(reduceField({ values: [1, 2], numeric: true }, ["nonsense"])).toEqual({});
  });

  it("reads a boolean as 1 or 0", () => {
    expect(calcValueOf(true)).toBe(1);
    expect(calcValueOf(false)).toBe(0);
  });
});
