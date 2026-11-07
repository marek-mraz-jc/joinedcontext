import type { Row } from "@joinedcontext/sdk";

const STATIONS = [
  { local: "kallio", name: "Kallio", base: 9 },
  { local: "kamppi", name: "Kamppi", base: 14 },
  { local: "mannerheimintie", name: "Mannerheimintie", base: 21, rising: true },
  { local: "makelankatu", name: "Mäkelänkatu", base: 17 },
  { local: "vartiokyla", name: "Vartiokylä", base: 6 },
  { local: "tapanila", name: "Tapanila", base: 8, silent: true },
];

function minutesBefore(now: Date, minutes: number): string {
  return new Date(now.getTime() - minutes * 60000).toISOString();
}

function id(local: string): string {
  return `urn:ngsi-ld:AirQualityObserved:hel.fi:air:${local}`;
}

/** Each station's current reading as the endpoint answers it: one over the level, one silent for 40 minutes. */
export function current(now: Date): Row[] {
  return STATIONS.map((station) => ({
    id: id(station.local),
    type: "AirQualityObserved",
    stationName: station.name,
    pm25: station.rising ? 31.4 : station.base + 0.6,
    no2: Math.round(station.base * 2.1),
    dateObserved: minutesBefore(now, station.silent ? 40 : 1),
  }));
}

/** The last hour, one reading every five minutes, as the broker's `temporalValues` bodies. */
export function lastHour(now: Date): { id: string; type: string; [attr: string]: unknown }[] {
  return STATIONS.map((station) => ({
    id: id(station.local),
    type: "AirQualityObserved",
    pm25: {
      type: "Property",
      values: Array.from({ length: 12 }, (_, step) => {
        const minutes = 60 - step * 5 + (station.silent ? 40 : 1);
        const wave = Math.round(Math.sin(step / 2 + station.base) * 20) / 10;
        const value = station.rising ? station.base + step * 0.35 : station.base + wave;
        return [Math.round(value * 10) / 10, minutesBefore(now, minutes)];
      }),
    },
  }));
}

export const ROWS: Row[] = current(new Date());
export const TEMPORAL = lastHour(new Date());
