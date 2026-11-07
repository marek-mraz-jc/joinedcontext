/**
 * The rules the screen draws with, without a map or a network (T-2435).
 *
 * The one that matters most is the order: staleness is decided before the thresholds, so a
 * station that stopped reporting at a clean value is never drawn as clean air.
 */
import { describe, expect, it } from "vitest";
import { toRichRow } from "@joinedcontext/sdk";
import {
  ageOf,
  bandOf,
  BAND_COLOUR,
  localIdOf,
  pointOf,
  PM10_HIGH,
  PM10_RAISED,
  STALE_AFTER_HOURS,
  stationsOf,
  toStation,
} from "./stations";
import { answer, station } from "./fixtures/verejne";

const NOW = new Date("2026-09-20T09:00:00Z");
const rowOf = (entity: Record<string, unknown>) => toRichRow(entity, "sk");

describe("one station, read off the endpoint's own answer", () => {
  it("takes the id, the place, both numbers and when the reading was taken", () => {
    const one = toStation(
      rowOf(station({ localId: "station-1", minutesAgo: 10, pm10: 18.4, pm25: 11.2, coordinates: [19.1462, 48.7359] }, NOW)),
    );
    expect(one.localId).toBe("station-1");
    expect(one.coordinates).toEqual([19.1462, 48.7359]);
    expect(one.pm10).toBe(18.4);
    expect(one.pm25).toBe(11.2);
    expect(ageOf(one.at, NOW)).toBe(10 * 60_000);
  });

  it("reads a station that publishes neither a place nor a value without inventing either", () => {
    const one = toStation(rowOf(station({ localId: "station-4", minutesAgo: 8 }, NOW)));
    expect(one.coordinates).toBeNull();
    expect(one.pm10).toBeNull();
    expect(one.pm25).toBeNull();
  });

  it("keeps the whole id when it is not a five-segment urn, rather than guessing a name", () => {
    expect(localIdOf("urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:banskabystrica-verejne:station-1")).toBe("station-1");
    expect(localIdOf("station-1")).toBe("station-1");
  });

  it("takes a Point and nothing else as a place", () => {
    expect(pointOf({ kind: "geo", value: { type: "Point", coordinates: [19.1, 48.7] } })).toEqual([19.1, 48.7]);
    expect(pointOf({ kind: "geo", value: { type: "Polygon", coordinates: [[[19.1, 48.7]]] } })).toBeNull();
    expect(pointOf({ kind: "geo", value: { type: "Point", coordinates: ["19.1", 48.7] } })).toBeNull();
    expect(pointOf(undefined)).toBeNull();
  });
});

describe("how a reading becomes a band", () => {
  const bandFor = (one: Parameters<typeof station>[0]) => bandOf(toStation(rowOf(station(one, NOW))), NOW);

  it("is within the limit below the elevated threshold", () => {
    expect(bandFor({ localId: "a", minutesAgo: 5, pm10: PM10_RAISED - 0.1 })).toBe("low");
  });

  it("is elevated at the elevated threshold and above the limit at the limit", () => {
    expect(bandFor({ localId: "a", minutesAgo: 5, pm10: PM10_RAISED })).toBe("raised");
    expect(bandFor({ localId: "a", minutesAgo: 5, pm10: PM10_HIGH - 0.1 })).toBe("raised");
    expect(bandFor({ localId: "a", minutesAgo: 5, pm10: PM10_HIGH })).toBe("high");
  });

  it("calls an old reading old, however clean the number on it is", () => {
    // The whole point of the screen: a station that stopped reporting at 9 µg/m³ must not be
    // drawn in the colour of clean air.
    const stale = bandFor({ localId: "a", minutesAgo: STALE_AFTER_HOURS * 60 + 1, pm10: 9 });
    expect(stale).toBe("stale");
    expect(BAND_COLOUR.stale).not.toBe(BAND_COLOUR.low);
  });

  it("is still fresh exactly at the boundary", () => {
    expect(bandFor({ localId: "a", minutesAgo: STALE_AFTER_HOURS * 60, pm10: 9 })).toBe("low");
  });

  it("says a station with no reading at all has none, and one with no timestamp is old", () => {
    expect(bandFor({ localId: "a", minutesAgo: 5 })).toBe("unknown");
    expect(bandFor({ localId: "a", pm10: 9 })).toBe("stale");
  });
});

describe("the list of stations", () => {
  it("opens on the freshest reading and puts the ones with no timestamp last", () => {
    const rows = answer(NOW).map(rowOf);
    const order = stationsOf(rows, NOW).map((one) => one.localId);
    expect(order).toEqual(["station-4", "station-1", "station-2", "station-3"]);
  });

  it("is empty for an empty answer rather than throwing", () => {
    expect(stationsOf([], NOW)).toEqual([]);
  });
});

describe("a station's odd shapes", () => {
  const ID = "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:banskabystrica-verejne:x";
  const one = (attrs: Record<string, unknown>) => toStation(rowOf({ id: ID, type: "AirQualityObserved", ...attrs }));

  it("takes the first of several values, and no place from a point that is not a number", () => {
    expect(one({ pm10: [{ type: "Property", value: 12, datasetId: "urn:a" }, { type: "Property", value: 40, datasetId: "urn:b" }] }).pm10).toBe(12);
    expect(one({ location: { type: "GeoProperty", value: { type: "Point", coordinates: [Infinity, 48.7] } } }).coordinates).toBeNull();
  });

  it("has no age for a time that is no time, and lists stations without one last", () => {
    expect(ageOf("someday", NOW)).toBeNull();
    const stations = stationsOf(
      [rowOf({ id: `${ID}-a`, type: "AirQualityObserved" }), rowOf(station({ localId: "b", minutesAgo: 5, pm10: 1 }, NOW)), rowOf({ id: `${ID}-c`, type: "AirQualityObserved" })],
      NOW,
    );
    expect(stations[0].localId).toBe("b");
  });
});

