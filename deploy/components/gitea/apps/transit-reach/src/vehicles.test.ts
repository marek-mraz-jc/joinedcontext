import { describe, expect, it } from "vitest";
import type { TemporalRow } from "@joinedcontext/sdk";
import { CENTRE, EMPTY, pointOf, readView, toVehicles, writeView } from "./vehicles";

describe("the view in the address", () => {
  it("reads a point in the capital region and an offered window, else the defaults", () => {
    expect(readView("")).toEqual(EMPTY);
    expect(readView("?at=24.95,60.2&hours=6&lang=fi")).toEqual({ at: { lon: 24.95, lat: 60.2 }, hours: 6 });
    expect(readView("?at=2.35,48.85&hours=5")).toEqual(EMPTY);
    expect(pointOf("24.9,60.1,3")).toBeNull();
    expect(pointOf("x,60")).toBeNull();
    expect(pointOf(null)).toBeNull();
  });

  it("writes only what differs from the default and keeps the language", () => {
    expect(writeView("?lang=en", EMPTY)).toBe("?lang=en");
    expect(writeView("?lang=en", { at: { lon: 24.95, lat: 60.2 }, hours: 1 })).toBe("?lang=en&at=24.95000%2C60.20000&hours=1");
    expect(writeView("", { at: CENTRE, hours: 3 })).toBe("");
  });
});

describe("the readings handed to the module", () => {
  const at = (minute: number) => new Date(Date.UTC(2030, 9, 7, 6, minute)).toISOString();
  const row = (series: TemporalRow["series"]): TemporalRow => ({ id: "v", type: "Vehicle", series });

  it("joins position, speed and route by time, the route in force carried forward", () => {
    const [vehicle] = toVehicles([
      row({
        location: [
          { value: { type: "Point", coordinates: [24.9, 60.2] }, observedAt: at(2) },
          { value: { type: "Point", coordinates: [24.91, 60.2] }, observedAt: at(0) },
          { value: { type: "Point", coordinates: [24.92, 60.2] }, observedAt: at(4) },
        ],
        speed: [
          { value: 0, observedAt: at(0) },
          { value: 7, observedAt: at(2) },
        ],
        route: [
          { value: "550", observedAt: at(1) },
          { value: "560", observedAt: at(4) },
        ],
      }),
    ]);
    expect(vehicle.points).toEqual([
      { t: Date.parse(at(0)), lon: 24.91, lat: 60.2, speed: 0 },
      { t: Date.parse(at(2)), lon: 24.9, lat: 60.2, speed: 7, route: "550" },
      { t: Date.parse(at(4)), lon: 24.92, lat: 60.2, route: "560" },
    ]);
  });

  it("drops what is not a point, and a vehicle with no history is one with no readings", () => {
    const [vehicle, empty] = toVehicles([
      row({
        location: [
          { value: { type: "LineString", coordinates: [[24.9, 60.2]] }, observedAt: at(0) },
          { value: "24.9,60.2", observedAt: at(1) },
          { value: { type: "Point", coordinates: [24.9, 60.2] }, observedAt: "never" },
        ],
      }),
      row({}),
    ]);
    expect(vehicle.points).toEqual([]);
    expect(empty).toEqual({ id: "v", points: [] });
  });
});
