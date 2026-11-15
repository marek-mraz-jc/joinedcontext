import type { Row } from "@joinedcontext/sdk";

const HOUR = 3_600_000;
/** The fixtures' clock: Sunday 20 October 2030, 09:00 in Helsinki. */
export const NOW = Date.UTC(2030, 9, 20, 6);

const place = (type: string, local: string, name: string, lon: number, lat: number): Row => {
  const fields: Record<string, unknown> = {
    name: { languageMap: { fi: name } },
    location: { type: "Point", coordinates: [lon, lat] },
    dateObserved: new Date(NOW).toISOString(),
  };
  return { id: `urn:ngsi-ld:${type}:hel.fi:helsinki:${local}`, type, ...fields } as Row;
};

/** Three air quality stations, one with no history, as HSY's feed is written. */
export const AIR_STATIONS: Row[] = [
  place("AirQualityObserved", "kallio", "Kallio 2", 24.9506, 60.1872),
  place("AirQualityObserved", "makelankatu", "Mäkelänkatu", 24.9519, 60.1963),
  place("AirQualityObserved", "vartiokyla", "Vartiokylä", 25.1044, 60.2236),
];

/** Three weather stations: Kaisaniemi near Kallio, a road station, the airport far out. */
export const WEATHER_STATIONS: Row[] = [
  place("WeatherObserved", "fmi-100971", "Helsinki Kaisaniemi", 24.9446, 60.1752),
  place("WeatherObserved", "road-1002", "kt51_Hki_Lapinlahti", 24.8985, 60.1669),
  place("WeatherObserved", "fmi-100968", "Vantaa Helsinki-Vantaa airport", 24.9727, 60.3294),
];

/** `values(h)` for each of the 72 hours before NOW, as the broker's temporalValues pairs. */
function hourly(values: (h: number) => number): Array<[number, string]> {
  return Array.from({ length: 72 }, (_, i) => {
    const h = 72 - i;
    return [values(h), new Date(NOW - h * HOUR).toISOString()] as [number, string];
  });
}

/** The wind of hour `h` before now: a slow swing between 1 and 9 m/s. */
const wind = (h: number) => Math.round((5 + 4 * Math.sin(h / 6)) * 10) / 10;

/**
 * Three days of hourly history: at Kallio PM2.5 falls as Kaisaniemi's wind rises, PM10 follows it
 * more loosely, and one PM2.5 reading is far from the rest; humidity is a share, as stored.
 */
export const HISTORY = [
  {
    id: AIR_STATIONS[0].id,
    type: "AirQualityObserved",
    pm25: { type: "Property", values: hourly((h) => (h === 30 ? 95 : Math.round((22 - 2 * wind(h)) * 10) / 10)) },
    pm10: { type: "Property", values: hourly((h) => Math.round((30 - 2 * wind(h) + ((h * 7) % 5)) * 10) / 10) },
    airQualityIndex: { type: "Property", values: hourly((h) => 2 + (h % 2)) },
  },
  {
    id: AIR_STATIONS[1].id,
    type: "AirQualityObserved",
    pm25: { type: "Property", values: hourly((h) => 8 + (h % 3)) },
  },
  {
    id: WEATHER_STATIONS[0].id,
    type: "WeatherObserved",
    windSpeed: { type: "Property", values: hourly(wind) },
    temperature: { type: "Property", values: hourly((h) => Math.round((8 + 3 * Math.cos(h / 4)) * 10) / 10) },
    relativeHumidity: { type: "Property", values: hourly((h) => 0.7 + (h % 4) / 20) },
  },
];
