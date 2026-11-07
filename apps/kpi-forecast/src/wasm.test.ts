import { describe, expect, it } from "vitest";
import { parseAnswer } from "./analysis";
import { HISTORY, KPIS, SPIKE } from "./fixtures/kpis";
import { toSeries } from "./kpis";
import { inProcess } from "./test-analyser";
import type { TemporalRow } from "@joinedcontext/sdk";

/** The fixture's temporal bodies as the SDK reads them. */
const rows: TemporalRow[] = HISTORY.map((h) => ({
  id: h.id,
  type: h.type,
  series: { currentValue: h.currentValue.values.map(([value, observedAt]) => ({ value, observedAt: String(observedAt) })) },
}));

// The page and the compiled module agree on the shape they exchange (AP-142): the series as the
// page hands them over, the answer as the page reads it.
describe("the compiled forecast", () => {
  it("finds the rhythm, the trend and the one point that looks wrong, per indicator", async () => {
    const out = await inProcess({ series: toSeries(KPIS, rows) });
    const byId = new Map(out.results.map((r) => [r.id.split(":").pop(), r]));
    const bikes = byId.get("bikes-available-sum")!;
    expect(bikes.status).toBe("ok");
    expect(bikes.season).toBe(24);
    expect(bikes.anomalies.map((a) => a.t)).toEqual([SPIKE]);
    expect(bikes.forecast).toHaveLength(24);
    expect(byId.get("bike-network-stations")!.trend!.direction).toBe("up");
    expect(byId.get("bike-network-stations")!.season).toBe(7);
    expect(byId.get("demo-counter-1")!.trend!.direction).toBe("down");
    expect(byId.get("bike-network-virtual")!.status).toBe("empty");
    expect({ rising: out.rising, falling: out.falling, flat: out.flat, short: out.short, anomalies: out.anomalies }).toEqual({
      rising: 1,
      falling: 1,
      flat: 1,
      short: 1,
      anomalies: 1,
    });
  });

  it("forecasts inside a band that holds the value it expects", async () => {
    const out = await inProcess({ series: toSeries(KPIS, rows), horizon: 5 });
    for (const result of out.results.filter((r) => r.status === "ok")) {
      expect(result.forecast).toHaveLength(5);
      for (const step of result.forecast) expect(step.lo <= step.v && step.v <= step.hi).toBe(true);
    }
  });

  it("answers an error for what it cannot read, and the page throws it", () => {
    expect(() => parseAnswer(JSON.stringify({ error: "the series could not be read: x" }))).toThrow("the series could not be read");
  });
});
