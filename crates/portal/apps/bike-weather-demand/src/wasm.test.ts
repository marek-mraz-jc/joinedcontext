import { describe, expect, it } from "vitest";
import wasm from "../wasm/pkg/bike_weather_demand_bg.wasm?url&inline";
import { estimate, initSync } from "../wasm/pkg/bike_weather_demand.js";
import { parseAnswer } from "./estimate";
import type { Cell, TemporalPoint } from "@joinedcontext/sdk";
import { toInput } from "./bikes";
import { ENTITIES, NOW, TEMPORAL } from "./fixtures/bikes";

const base64 = wasm.slice(wasm.indexOf(",") + 1);
initSync({ module: Uint8Array.from(atob(base64), (c) => c.charCodeAt(0)) });

function extractPoints(attr: unknown): TemporalPoint[] {
  if (!attr) return [];
  if (Array.isArray(attr)) {
    return attr.map((entry: unknown) => {
      if (Array.isArray(entry)) return { value: entry[0] as Cell, observedAt: String(entry[1]) };
      return entry as TemporalPoint;
    });
  }
  if (typeof attr === "object" && attr !== null) {
    const values = (attr as { values?: unknown[] }).values;
    if (Array.isArray(values)) {
      return values.map((entry: unknown) => {
        if (Array.isArray(entry)) return { value: entry[0] as Cell, observedAt: String(entry[1]) };
        return entry as TemporalPoint;
      });
    }
  }
  return [];
}

describe("wasm estimate on fixtures", () => {
  const bikeStation =
    ENTITIES.find((e) => e.type === "BikeHireDockingStation" && (e.totalSlotNumber === 30 || e.totalSlotNumber === "30")) ??
    ENTITIES.find((e) => e.type === "BikeHireDockingStation");
  const totalSlots = Number(bikeStation?.totalSlotNumber ?? 30);
  const stationId = bikeStation?.id;

  const bikeTemporal =
    TEMPORAL.find((t) => (stationId ? t.id === stationId : t.type === "BikeHireDockingStation")) ??
    TEMPORAL.find((t) => t.type === "BikeHireDockingStation");
  const weatherTemporal = TEMPORAL.find((t) => t.type === "WeatherObserved");

  const bikePoints = extractPoints((bikeTemporal as Record<string, unknown> | undefined)?.availableBikeNumber);
  const tempPoints = extractPoints((weatherTemporal as Record<string, unknown> | undefined)?.temperature);
  const precPoints = extractPoints((weatherTemporal as Record<string, unknown> | undefined)?.precipitation);

  const input = toInput({
    nowSec: NOW,
    totalSlots,
    bikePoints,
    tempPoints,
    precPoints,
  });

  const rawAnswer = estimate(JSON.stringify(input));
  const out = parseAnswer(rawAnswer);

  it("recovers 0.5 per degree and -2 for rain within 1e-3, with enough true", () => {
    expect(out.enough).toBe(true);
    expect(out.weather).toBeDefined();
    expect(out.weather).not.toBeNull();
    expect(out.weather?.perDegree).toBeCloseTo(0.5, 3);
    expect(out.weather?.rain).toBeCloseTo(-2.0, 3);
  });

  it("produces six estimate hours clamped to 0..=30 with low <= mean <= high", () => {
    expect(out.estimate).toHaveLength(6);
    for (const hour of out.estimate) {
      expect(hour.mean).toBeGreaterThanOrEqual(0);
      expect(hour.mean).toBeLessThanOrEqual(30);
      expect(hour.low).toBeGreaterThanOrEqual(0);
      expect(hour.high).toBeLessThanOrEqual(30);
      expect(hour.low).toBeLessThanOrEqual(hour.mean);
      expect(hour.mean).toBeLessThanOrEqual(hour.high);
    }
  });

  it("yields enough false and no weather when history is too short (12 hours)", () => {
    const shortBikePoints = bikePoints.slice(0, 12);
    const shortTempPoints = tempPoints.slice(0, 12);
    const shortPrecPoints = precPoints.slice(0, 12);

    const shortInput = toInput({
      nowSec: NOW,
      totalSlots,
      bikePoints: shortBikePoints,
      tempPoints: shortTempPoints,
      precPoints: shortPrecPoints,
    });

    const shortRaw = estimate(JSON.stringify(shortInput));
    const shortOut = parseAnswer(shortRaw);

    expect(shortOut.enough).toBe(false);
    expect(shortOut.weather).toBeNull();
    expect(shortOut.estimate).toHaveLength(0);
  });

  it("returns an error object on bad JSON and parseAnswer throws", () => {
    const badRaw = estimate("not valid json");
    const parsed = JSON.parse(badRaw) as { error?: string };

    expect(parsed.error).toBeDefined();
    expect(parsed.error).toContain("the estimate could not be read");
    expect(() => parseAnswer(badRaw)).toThrow("the estimate could not be read");
  });
});
