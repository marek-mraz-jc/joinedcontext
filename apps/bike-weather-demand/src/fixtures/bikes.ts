import type { Row, TemporalPoint } from "@joinedcontext/sdk";

/** Fixed Unix seconds: Thursday noon in October 2026 (2026-10-15T12:00:00Z). */
export const NOW = 1792065600;

export const STATION_1: Row = {
  id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:station-1",
  type: "BikeHireDockingStation",
  name: "Rautatientori",
  location: { type: "Point", coordinates: [24.9384, 60.1699] },
  availableBikeNumber: 15,
  freeSlotNumber: 15,
  totalSlotNumber: 30,
  status: "working",
  dateModified: "2026-10-15T12:00:00Z",
};

export const STATION_2: Row = {
  id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:station-2",
  type: "BikeHireDockingStation",
  name: "Hakaniemi",
  location: { type: "Point", coordinates: [24.95, 60.178] },
  availableBikeNumber: 10,
  freeSlotNumber: 10,
  totalSlotNumber: 20,
  status: "working",
  dateModified: "2026-10-15T12:00:00Z",
};

export const STATION_3: Row = {
  id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:station-3",
  type: "BikeHireDockingStation",
  name: "Töölöntori",
  location: { type: "Point", coordinates: [24.92, 60.175] },
  availableBikeNumber: 6,
  freeSlotNumber: 6,
  totalSlotNumber: 12,
  status: "working",
  dateModified: "2026-10-15T12:00:00Z",
};

/** Nearest weather station: ~1.00 km from station 1. */
export const WEATHER_1: Row = {
  id: "urn:ngsi-ld:WeatherObserved:hel.fi:helsinki:weather-1",
  type: "WeatherObserved",
  name: "Kaisaniemi",
  location: { type: "Point", coordinates: [24.9384, 60.1789] },
  temperature: 14.8,
  precipitation: 0.0,
  dateObserved: "2026-10-15T12:00:00Z",
};

/** Distant weather station: ~20.01 km from station 1. */
export const WEATHER_2: Row = {
  id: "urn:ngsi-ld:WeatherObserved:hel.fi:helsinki:weather-2",
  type: "WeatherObserved",
  name: "Kumpula etäsää",
  location: { type: "Point", coordinates: [24.9384, 60.3499] },
  temperature: 12.0,
  precipitation: 1.0,
  dateObserved: "2026-10-15T12:00:00Z",
};

const station1BikeValues: [number, string][] = [];
const weather1TempValues: [number, string][] = [];
const weather1PrecValues: [number, string][] = [];

// 168 hours of historical data up to and including NOW
for (let step = 1; step <= 168; step++) {
  const ts = NOW - (168 - step) * 3600;
  const iso = new Date(ts * 1000).toISOString();

  // Europe/Helsinki hour of day in summer time (EEST, UTC+3)
  const helsinkiHour = ((Math.floor((ts + 3 * 3600) / 3600) % 24) + 24) % 24;
  const profileHod = 12.0 + (helsinkiHour % 6);

  // Temperature varies deterministically across hours and days (not collinear with hour of day)
  const temp = 10.0 + ((step * 7) % 15) * 0.8;

  // Rain occurs on selected hours
  const raining = step % 17 === 3 || step % 23 === 5;
  const prec = raining ? 2.0 : 0.0;
  const rainTerm = raining ? 1.0 : 0.0;

  // deterministic: profile(hour of day) + 0.5 * temperature - 2.0 * rain
  const bikes = Number((profileHod + 0.5 * temp - 2.0 * rainTerm).toFixed(2));

  station1BikeValues.push([bikes, iso]);
  weather1TempValues.push([temp, iso]);
  weather1PrecValues.push([prec, iso]);
}

export const BIKE_TEMPORAL_POINTS: TemporalPoint[] = station1BikeValues.map(([value, observedAt]) => ({
  value,
  observedAt,
}));

export const TEMP_TEMPORAL_POINTS: TemporalPoint[] = weather1TempValues.map(([value, observedAt]) => ({
  value,
  observedAt,
}));

export const PREC_TEMPORAL_POINTS: TemporalPoint[] = weather1PrecValues.map(([value, observedAt]) => ({
  value,
  observedAt,
}));

export const ENTITIES: Row[] = [STATION_1, STATION_2, STATION_3, WEATHER_1, WEATHER_2];

export const TEMPORAL = [
  {
    id: STATION_1.id,
    type: "BikeHireDockingStation",
    availableBikeNumber: {
      type: "Property",
      values: station1BikeValues,
    },
  },
  {
    id: STATION_2.id,
    type: "BikeHireDockingStation",
    availableBikeNumber: {
      type: "Property",
      values: [],
    },
  },
  {
    id: WEATHER_1.id,
    type: "WeatherObserved",
    temperature: {
      type: "Property",
      values: weather1TempValues,
    },
    precipitation: {
      type: "Property",
      values: weather1PrecValues,
    },
  },
];
