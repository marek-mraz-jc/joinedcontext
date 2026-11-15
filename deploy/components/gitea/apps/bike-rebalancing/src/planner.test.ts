import { describe, expect, it } from "vitest";
import { STATIONS } from "./fixtures/stations";
import { computePlan, planHere, readAnswer } from "./planner";
import { stationsOf } from "./stations";

// The real WebAssembly planner (test-setup.ts loads it), called as the page calls it.
describe("planner", () => {
  it("assesses every station and routes the van from full to empty within its capacity", async () => {
    const plan = await planHere({ stations: stationsOf(STATIONS), settings: { vanCapacity: 10 } });
    const level = Object.fromEntries(plan.needs.map((need) => [need.id.split(":").pop(), need.level]));
    expect(level).toEqual({
      "001": "high",
      "002": "empty",
      "003": "full",
      "004": "unknown",
      "005": "empty",
      "006": "low",
      "007": "balanced",
      "008": "unknown",
    });
    expect(plan.route.moved).toBeGreaterThan(0);
    expect(plan.route.stops.every((stop) => stop.load >= 0 && stop.load <= 10)).toBe(true);
    const ids = plan.route.stops.map((stop) => stop.id.split(":").pop());
    expect(ids).not.toContain("008");
    expect(ids).not.toContain("004");
  });

  it("leaves out what the operator leaves out", async () => {
    const skipped = STATIONS[1].id;
    const plan = await computePlan({ stations: stationsOf(STATIONS), settings: { exclude: [skipped] } });
    expect(plan.route.stops.map((stop) => stop.id)).not.toContain(skipped);
  });

  it("plans nothing for no station", async () => {
    const plan = await planHere({ stations: [], settings: {} });
    expect(plan).toEqual({ needs: [], route: { stops: [], km: 0, moved: 0 } });
  });

  it("says in words what the planner could not read", () => {
    expect(() => readAnswer(JSON.stringify({ error: "the stations could not be read: x" }))).toThrow("the stations could not be read");
  });
});
