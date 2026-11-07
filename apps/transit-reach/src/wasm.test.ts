import { describe, expect, it } from "vitest";
import type { TemporalRow } from "@joinedcontext/sdk";
import { parseAnswer } from "./analysis";
import { HISTORY, STOPS } from "./fixtures/vehicles";
import { CENTRE, toVehicles } from "./vehicles";
import { inProcess } from "./test-analyser";

/** The fixture's temporal bodies as the SDK reads them. */
const rows: TemporalRow[] = HISTORY.map((h) => ({
  id: h.id,
  type: h.type,
  series: {
    location: h.location.values.map(([value, observedAt]) => ({ value: value as { type: string; coordinates: unknown }, observedAt: String(observedAt) })),
    speed: h.speed.values.map(([value, observedAt]) => ({ value: value as number, observedAt: String(observedAt) })),
    route: h.route.values.map(([value, observedAt]) => ({ value: value as string, observedAt: String(observedAt) })),
  },
}));

// The page and the compiled module agree on the shape they exchange (AP-142).
describe("the compiled reach", () => {
  it("derives the four stops and the rides, and rides east and changes north within the bands", async () => {
    const out = await inProcess({ vehicles: toVehicles(rows), origin: CENTRE });
    expect(out.vehicles).toBe(5);
    expect(out.stops).toHaveLength(4);
    expect(out.routes).toEqual(["4570", "550"]);
    // Rautatientori is where we stand; 2 km east is a wait and a ride away, 4 km east a ride
    // further, north a change and another ride.
    const near = (stop: { lon: number; lat: number }, at: readonly [number, number]) => Math.abs(stop.lon - at[0]) < 0.001 && Math.abs(stop.lat - at[1]) < 0.001;
    const minutes = (at: readonly [number, number]) => out.stops.find((s) => near(s, at))?.minutes;
    // 5 min wait, 4 min rides; the change at 4 km east waits again.
    expect(minutes(STOPS.centre)).toBe(0);
    expect(minutes(STOPS.east2)).toBe(9);
    expect(minutes(STOPS.east4)).toBe(13);
    expect(minutes(STOPS.north)).toBe(22);
    expect(out.bands.map((b) => b.stops)).toEqual([2, 3, 4]);
    expect(out.bands.map((b) => b.minutes)).toEqual([10, 20, 30]);
    expect(out.bands[0].areaKm2).toBeLessThan(out.bands[2].areaKm2);
  });

  it("answers walking alone with no vehicles", async () => {
    const out = await inProcess({ vehicles: [], origin: CENTRE });
    expect(out.stops).toEqual([]);
    expect(out.bands[2].areaKm2).toBeGreaterThan(5);
  });

  it("answers an error for what it cannot read, and the page throws it", async () => {
    await expect(inProcess({ vehicles: [], origin: { lon: 500, lat: 0 } })).rejects.toThrow("not on the globe");
    expect(() => parseAnswer(JSON.stringify({ error: "x" }))).toThrow("x");
  });
});
