import { describe, expect, it } from "vitest";
import type { Row, TemporalPoint } from "@joinedcontext/sdk";
import {
  defaultStationId,
  haversineKm,
  nearestWeatherStation,
  readHash,
  daysAgoIso,
  stationBikes,
  stationFreeSlots,
  stationName,
  stationSlots,
  toInput,
  writeHash,
} from "./bikes";

describe("nearestWeatherStation", () => {
  const bikeStation: Row = {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001",
    type: "BikeHireDockingStation",
    name: "Rautatientori",
    location: { type: "Point", coordinates: [24.94, 60.17] },
  };

  it("finds the nearest weather station within 10 km", () => {
    const nearWeather: Row = {
      id: "urn:ngsi-ld:WeatherObserved:hel.fi:helsinki:near",
      type: "WeatherObserved",
      name: "Kaisaniemi",
      location: { type: "Point", coordinates: [24.94, 60.18] },
    };
    const farWeather: Row = {
      id: "urn:ngsi-ld:WeatherObserved:hel.fi:helsinki:far",
      type: "WeatherObserved",
      name: "Vantaa",
      location: { type: "Point", coordinates: [24.94, 60.35] },
    };

    const match = nearestWeatherStation(bikeStation, [nearWeather, farWeather]);
    expect(match).not.toBeNull();
    expect(match?.station.id).toBe("urn:ngsi-ld:WeatherObserved:hel.fi:helsinki:near");
    expect(match?.distanceKm).toBeCloseTo(1.11, 1);
    expect(match!.distanceKm).toBeLessThan(10);
  });

  it("returns null when all weather stations are beyond 10 km", () => {
    const farWeather: Row = {
      id: "urn:ngsi-ld:WeatherObserved:hel.fi:helsinki:far",
      type: "WeatherObserved",
      name: "Vantaa",
      location: { type: "Point", coordinates: [24.94, 60.35] },
    };

    const match = nearestWeatherStation(bikeStation, [farWeather]);
    expect(match).toBeNull();
  });

  it("returns null when bike station or weather stations lack valid coordinates", () => {
    const unlocatedBike: Row = {
      id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:none",
      type: "BikeHireDockingStation",
      location: null,
    };
    const nearWeather: Row = {
      id: "urn:ngsi-ld:WeatherObserved:hel.fi:helsinki:near",
      type: "WeatherObserved",
      location: { type: "Point", coordinates: [24.94, 60.18] },
    };

    expect(nearestWeatherStation(unlocatedBike, [nearWeather])).toBeNull();

    const unlocatedWeather: Row = {
      id: "urn:ngsi-ld:WeatherObserved:hel.fi:helsinki:none",
      type: "WeatherObserved",
      location: null,
    };
    expect(nearestWeatherStation(bikeStation, [unlocatedWeather])).toBeNull();
  });
});

describe("haversineKm", () => {
  it("calculates zero distance for identical coordinates", () => {
    expect(haversineKm(24.94, 60.17, 24.94, 60.17)).toBe(0);
  });

  it("calculates expected distance across Helsinki area", () => {
    const dist = haversineKm(24.94, 60.17, 24.65, 60.20);
    expect(dist).toBeGreaterThan(15);
    expect(dist).toBeLessThan(18);
  });
});

describe("station properties and defaults", () => {
  it("formats stationName, stripping URN or falling back to id", () => {
    expect(stationName({ id: "s1", type: "BikeHireDockingStation", name: "Kamppi" })).toBe("Kamppi");
    expect(stationName({ id: "s2", type: "BikeHireDockingStation", name: "urn:ngsi-ld:BikeHireDockingStation:s2" })).toBe("s2");
    expect(stationName({ id: "s3", type: "BikeHireDockingStation", name: "" })).toBe("s3");
    expect(stationName({ id: "s4", type: "BikeHireDockingStation", name: null })).toBe("s4");
  });

  it("reads numeric slots, available bikes, and free slots", () => {
    const row: Row = {
      id: "s1",
      type: "BikeHireDockingStation",
      totalSlotNumber: 25,
      availableBikeNumber: 10,
      freeSlotNumber: 15,
    };
    expect(stationSlots(row)).toBe(25);
    expect(stationBikes(row)).toBe(10);
    expect(stationFreeSlots(row)).toBe(15);

    const invalid: Row = {
      id: "s2",
      type: "BikeHireDockingStation",
      totalSlotNumber: "invalid",
      availableBikeNumber: -4,
      freeSlotNumber: null,
    };
    expect(stationSlots(invalid)).toBe(0);
    expect(stationBikes(invalid)).toBe(0);
    expect(stationFreeSlots(invalid)).toBe(0);
  });

  it("defaultStationId selects the station with the most slots, breaking ties by name", () => {
    expect(defaultStationId([])).toBeNull();

    const s1: Row = { id: "s1", type: "BikeHireDockingStation", name: "Station B", totalSlotNumber: 20 };
    const s2: Row = { id: "s2", type: "BikeHireDockingStation", name: "Station A", totalSlotNumber: 30 };
    const s3: Row = { id: "s3", type: "BikeHireDockingStation", name: "Station C", totalSlotNumber: 10 };
    expect(defaultStationId([s1, s2, s3])).toBe("s2");

    const sTie1: Row = { id: "s-z", type: "BikeHireDockingStation", name: "Z-Station", totalSlotNumber: 30 };
    const sTie2: Row = { id: "s-a", type: "BikeHireDockingStation", name: "A-Station", totalSlotNumber: 30 };
    expect(defaultStationId([sTie1, sTie2])).toBe("s-a");
  });
});

describe("readHash and writeHash", () => {
  it("round trips station id in URL hash", () => {
    const urn = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001";
    const hash = writeHash(urn);
    expect(hash).toBe(`#station?id=${encodeURIComponent(urn)}`);
    expect(readHash(hash)).toEqual({ stationId: urn });
  });

  it("ignores junk and malformed parameters", () => {
    expect(readHash("")).toEqual({ stationId: null });
    expect(readHash("#")).toEqual({ stationId: null });
    expect(readHash("#station")).toEqual({ stationId: null });
    expect(readHash("#station?other=123")).toEqual({ stationId: null });
    expect(readHash("#station?id=")).toEqual({ stationId: null });
    expect(readHash("#station?id=   ")).toEqual({ stationId: null });
    expect(readHash("#station?id=valid-id&other=xyz")).toEqual({ stationId: "valid-id" });
  });
});

describe("daysAgoIso", () => {
  it("returns the exact ISO string 7 days prior to given date", () => {
    const fixed = new Date("2026-10-15T12:00:00.000Z");
    const result = daysAgoIso(7, fixed);
    expect(daysAgoIso(3, fixed)).toBe("2026-10-12T12:00:00.000Z");
    expect(result).toBe("2026-10-08T12:00:00.000Z");
    expect(fixed.getTime() - new Date(result).getTime()).toBe(7 * 24 * 60 * 60 * 1000);
  });

  it("defaults to 7 days before current time when called without arguments", () => {
    const before = Date.now();
    const result = daysAgoIso();
    const after = Date.now();
    const diff = before - new Date(result).getTime();
    expect(diff).toBeGreaterThanOrEqual(7 * 24 * 60 * 60 * 1000 - 50);
    expect(after - new Date(result).getTime()).toBeLessThanOrEqual(7 * 24 * 60 * 60 * 1000 + 50);
  });
});

describe("toInput", () => {
  it("drops non-numeric bike values while preserving valid ones", () => {
    const bikePoints: TemporalPoint[] = [
      { observedAt: "2026-10-01T10:00:00Z", value: 12 },
      { observedAt: "2026-10-01T11:00:00Z", value: "15" },
      { observedAt: "2026-10-01T12:00:00Z", value: "non-numeric" },
      { observedAt: "2026-10-01T13:00:00Z", value: null },
      { observedAt: "2026-10-01T14:00:00Z", value: NaN },
      { observedAt: "2026-10-01T15:00:00Z", value: Infinity },
      { observedAt: "2026-10-01T16:00:00Z", value: "" },
      { observedAt: "2026-10-01T17:00:00Z", value: 0 },
    ];

    const input = toInput({
      nowSec: 1_700_000_000,
      totalSlots: 30,
      bikePoints,
    });

    expect(input.now).toBe(1_700_000_000);
    expect(input.totalSlots).toBe(30);
    expect(input.bikes).toEqual([
      { at: "2026-10-01T10:00:00Z", value: 12 },
      { at: "2026-10-01T11:00:00Z", value: 15 },
      { at: "2026-10-01T17:00:00Z", value: 0 },
    ]);
  });

  it("merges temperature and precipitation points, dropping non-numeric values", () => {
    const tempPoints: TemporalPoint[] = [
      { observedAt: "2026-10-01T10:00:00Z", value: 14.5 },
      { observedAt: "2026-10-01T11:00:00Z", value: "bad-temp" },
      { observedAt: "2026-10-01T12:00:00Z", value: 16.0 },
    ];
    const precPoints: TemporalPoint[] = [
      { observedAt: "2026-10-01T10:00:00Z", value: 0.0 },
      { observedAt: "2026-10-01T11:00:00Z", value: 2.5 },
      { observedAt: "2026-10-01T13:00:00Z", value: 1.0 },
    ];

    const input = toInput({
      nowSec: 1_700_000_000,
      totalSlots: 20,
      bikePoints: [],
      tempPoints,
      precPoints,
    });

    expect(input.weather).toEqual([
      { at: "2026-10-01T10:00:00Z", temperature: 14.5, precipitation: 0.0 },
      { at: "2026-10-01T11:00:00Z", temperature: null, precipitation: 2.5 },
      { at: "2026-10-01T12:00:00Z", temperature: 16.0, precipitation: null },
      { at: "2026-10-01T13:00:00Z", temperature: null, precipitation: 1.0 },
    ]);
  });

  it("preserves pre-aligned weatherPoints directly when provided", () => {
    const weatherPoints = [{ at: "2026-10-01T10:00:00Z", temperature: 15.0, precipitation: 0.0 }];
    const input = toInput({
      nowSec: 1_700_000_000,
      totalSlots: 20,
      bikePoints: [],
      weatherPoints,
    });
    expect(input.weather).toBe(weatherPoints);
  });
});
