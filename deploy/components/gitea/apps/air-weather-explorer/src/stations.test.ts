import { describe, expect, it } from "vitest";
import type { Row } from "@joinedcontext/sdk";
import { kmBetween, localOf, nearest, stationsOf } from "./stations";

const station = (local: string, name: string, at: [number, number] | null) =>
  ({ id: `urn:ngsi-ld:WeatherObserved:hel.fi:helsinki:${local}`, type: "WeatherObserved", name, ...(at ? { location: { type: "Point", coordinates: at } } : {}) }) as Row;

describe("stations", () => {
  it("lists the stations by name, an unnamed one by its id", () => {
    const list = stationsOf([station("b", "Pasila", [24.93, 60.2]), station("a", "", null), station("c", "Kallio", [24.95, 60.18])]);
    expect(list.map((s) => s.name)).toEqual(["a", "Kallio", "Pasila"]);
    expect(list[0].at).toBeNull();
    expect(localOf("urn:ngsi-ld:WeatherObserved:hel.fi:helsinki:fmi-100971")).toBe("fmi-100971");
  });

  it("pairs a station with the nearest weather station that has a place", () => {
    const weather = stationsOf([station("airport", "Airport", [24.97, 60.33]), station("lost", "Lost", null), station("kaisaniemi", "Kaisaniemi", [24.94, 60.18])]);
    const kallio = stationsOf([station("kallio", "Kallio", [24.95, 60.19])])[0];
    const found = nearest(kallio, weather);
    expect(found?.station.local).toBe("kaisaniemi");
    expect(found?.km).toBeCloseTo(kmBetween([24.95, 60.19], [24.94, 60.18]), 6);
    expect(nearest(kallio, [])).toBeNull();
    const nowhere = stationsOf([station("x", "X", null)])[0];
    expect(nearest(nowhere, weather)?.km).toBeNull();
  });

  it("measures the great circle, across the antimeridian too", () => {
    expect(kmBetween([24.9446, 60.1752], [24.9727, 60.3294])).toBeCloseTo(17.2, 1);
    expect(kmBetween([180, 0], [-180, 0])).toBeCloseTo(0, 6);
  });
});
