import type { Row } from "@joinedcontext/sdk";

const STATIONS = [
  { local: "mannerheimintie", name: "Mannerheimintie", lon: 24.9384, lat: 60.1719, base: 14 },
  { local: "makelankatu", name: "Mäkelänkatu", lon: 24.9516, lat: 60.1963, base: 12 },
  { local: "kallio", name: "Kallio", lon: 24.9507, lat: 60.1872, base: 8 },
  { local: "vartiokyla", name: "Vartiokylä", lon: 25.0917, lat: 60.2231, base: 5 },
];

/**
 * Fourteen days of daily PM2.5 at four stations, a still, smoky spell in the middle of the second
 * week, as the endpoint answers them.
 */
export const ROWS: Row[] = STATIONS.flatMap((station) =>
  Array.from({ length: 14 }, (_, day) => {
    const spell = day >= 8 && day <= 10 ? 9 : 0;
    const wobble = ((day * 7 + station.base) % 5) - 2;
    const date = `2026-09-${String(day + 10).padStart(2, "0")}`;
    return {
      id: `urn:ngsi-ld:AirQualityObserved:hel.fi:air:${station.local}-${date}`,
      type: "AirQualityObserved",
      stationName: station.name,
      pm25: Math.round((station.base + spell + wobble) * 10) / 10,
      dateObserved: `${date}T12:00:00Z`,
      location: { type: "Point", coordinates: [station.lon, station.lat] },
    };
  }),
);
