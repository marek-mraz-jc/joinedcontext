import { describe, expect, it } from "vitest";
import { toInput } from "./alerts";
import { parseAnswer } from "./analysis";
import { ALERTS } from "./fixtures/alerts";
import { inProcess } from "./test-analyser";

// The page and the compiled module agree on the shape they exchange (AP-142): the alerts as the
// page hands them over, the answer as the page reads it.
describe("the compiled analysis", () => {
  it("bins, finds the repeat place and counts the hours on Helsinki's clock", async () => {
    const out = await inProcess({ alerts: ALERTS.map(toInput), filter: {} });
    expect(out.total).toBe(8);
    expect(out.kept).toBe(8);
    expect(out.untimed).toBe(1);
    expect(out.unlocated).toBe(0);
    expect(out.places).toHaveLength(1);
    expect(out.places[0].count).toBe(4);
    expect(out.hexes[0].count).toBe(4);
    // Mondays 05:00–05:45 UTC are 08:00 in Helsinki in October (summer time until the 27th).
    expect(out.busiest).toEqual({ weekday: 0, hour: 8, count: 4 });
    expect(out.subCategories.map((s) => s.name)).toEqual(["roadWorks", "carAccident"]);
  });

  it("filters over the whole set and keeps the choices", async () => {
    const out = await inProcess({ alerts: ALERTS.map(toInput), filter: { subCategories: ["carAccident"] } });
    expect(out.kept).toBe(3);
    expect(out.places).toEqual([]);
    expect(out.subCategories).toHaveLength(2);
  });

  it("says what is wrong instead of answering with nothing", () => {
    expect(() => parseAnswer(JSON.stringify({ error: "the alerts could not be read: x" }))).toThrow("the alerts could not be read");
  });
});
