/**
 * What a reading means, as pure functions over what the backend answered (T-2925): the index
 * band a station is drawn in, and one day of PM10 and PM2.5 as points a chart can draw. Tested
 * without a map, a browser or a network.
 */
import type { Station } from "./api";

/**
 * The Finnish Meteorological Institute's hourly air quality index (`AQINDEX_PT1H_avg`), which the
 * helsinki pipeline stores as `airQualityIndex`: 1 good … 5 very poor. `unknown` when a station
 * sends no index, never "good": a missing number is not clean air.
 */
export type Band = "good" | "satisfactory" | "fair" | "poor" | "veryPoor" | "unknown";

export const BANDS: Exclude<Band, "unknown">[] = ["good", "satisfactory", "fair", "poor", "veryPoor"];

export const BAND_LABEL: Record<Band, string> = {
  good: "Good",
  satisfactory: "Satisfactory",
  fair: "Fair",
  poor: "Poor",
  veryPoor: "Very poor",
  unknown: "No index",
};

/** The index scale's own colours, green to purple; the legend repeats each band in words (UI-30). */
export const BAND_COLOUR: Record<Band, string> = {
  good: "#1a9850",
  satisfactory: "#91cf60",
  fair: "#fdae61",
  poor: "#d73027",
  veryPoor: "#762a83",
  unknown: "#8c8c8c",
};

export function bandOf(index: number | undefined): Band {
  if (index === undefined || !Number.isFinite(index)) return "unknown";
  // The hourly average is a decimal; the band is the nearest step of the 1…5 scale.
  const step = Math.min(5, Math.max(1, Math.round(index)));
  return BANDS[step - 1];
}

/** Directive 2008/50/EC in µg/m³: the PM10 daily limit and the PM2.5 annual limit value. */
export const LIMITS = { pm10: 50, pm25: 25 } as const;

export type Pollutant = keyof typeof LIMITS;

export interface Point {
  at: string;
  /** Milliseconds since the epoch, for the time axis. */
  time: number;
  value: number;
}

export type History = Record<Pollutant, Point[]>;

/**
 * The temporal entity the backend forwards (`/api/stations/{id}/history`), normalized form: each
 * attribute an instance or a list of instances with `value` and `observedAt`. An instance without
 * a number or a time is dropped rather than drawn at zero or at the epoch.
 */
export function historyOf(entity: unknown): History {
  const record = (entity ?? {}) as Record<string, unknown>;
  const pointsOf = (attribute: unknown): Point[] => {
    const instances = Array.isArray(attribute) ? attribute : attribute ? [attribute] : [];
    return instances
      .map((instance) => instance as { value?: unknown; observedAt?: unknown })
      .flatMap((instance) => {
        const time = typeof instance.observedAt === "string" ? Date.parse(instance.observedAt) : NaN;
        return typeof instance.value === "number" && Number.isFinite(instance.value) && Number.isFinite(time)
          ? [{ at: instance.observedAt as string, time, value: instance.value }]
          : [];
      })
      .sort((a, b) => a.time - b.time);
  };
  return { pm10: pointsOf(record.pm10), pm25: pointsOf(record.pm25) };
}

/**
 * A station as a map point. Declared here rather than taken from the ambient `GeoJSON` namespace,
 * which only an install brings in: the build lane links the template's store and has none (T-2935).
 */
export type StationFeature = {
  type: "Feature";
  geometry: { type: "Point"; coordinates: [number, number] };
  properties: { id: string; name: string; colour: string };
};

export type StationCollection = { type: "FeatureCollection"; features: StationFeature[] };

/** The stations with a position, as GeoJSON points carrying their id, name and band. */
export function stationFeatures(stations: Station[]): StationCollection {
  return {
    type: "FeatureCollection",
    features: stations.flatMap((station) =>
      station.coordinates
        ? [
            {
              type: "Feature" as const,
              geometry: { type: "Point" as const, coordinates: station.coordinates },
              properties: {
                id: station.id,
                name: station.name ?? station.id,
                colour: BAND_COLOUR[bandOf(station.airQualityIndex)],
              },
            },
          ]
        : [],
    ),
  };
}
