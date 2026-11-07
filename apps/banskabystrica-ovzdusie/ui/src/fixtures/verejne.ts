/**
 * What the endpoint of `banskabystrica-verejne` answers, as the gateway shapes it (T-2435, T-2949).
 *
 * The timestamps are relative to a `now` the test passes, because what this fixture is for is
 * the difference between a reading taken minutes ago and one taken this morning: written as
 * literals they would all be stale a day after they were typed, and the staleness case would
 * pass for the wrong reason.
 */
const URN = "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:banskabystrica-verejne";

export interface StationFixture {
  localId: string;
  minutesAgo?: number;
  pm10?: number;
  pm25?: number;
  coordinates?: [number, number];
}

export function station(one: StationFixture, now: Date): Record<string, unknown> {
  const entity: Record<string, unknown> = {
    id: `${URN}:${one.localId}`,
    type: "AirQualityObserved",
  };
  if (one.minutesAgo !== undefined) {
    const at = new Date(now.getTime() - one.minutesAgo * 60_000).toISOString();
    entity.dateObserved = { type: "Property", value: at };
    entity.observedAt = { type: "Property", value: at };
  }
  if (one.pm10 !== undefined) {
    entity.pm10 = { type: "Property", value: one.pm10, unitCode: "GQ" };
  }
  if (one.pm25 !== undefined) {
    entity.pm25 = { type: "Property", value: one.pm25, unitCode: "GQ" };
  }
  if (one.coordinates) {
    entity.location = { type: "GeoProperty", value: { type: "Point", coordinates: one.coordinates } };
  }
  return entity;
}

/** The four stations the cases are written against: fresh and clean, fresh and over the limit,
 *  a low reading that stopped this morning, and one that publishes neither a place nor a value. */
export const STATIONS: StationFixture[] = [
  { localId: "station-1", minutesAgo: 12, pm10: 18.4, pm25: 11.2, coordinates: [19.1462, 48.7359] },
  { localId: "station-2", minutesAgo: 20, pm10: 61.5, pm25: 38.1, coordinates: [19.1533, 48.7411] },
  { localId: "station-3", minutesAgo: 5 * 60, pm10: 9.1, pm25: 4.4, coordinates: [19.1201, 48.7288] },
  { localId: "station-4", minutesAgo: 8 },
];

export function answer(now: Date, stations: StationFixture[] = STATIONS): Record<string, unknown>[] {
  return stations.map((one) => station(one, now));
}

/** A day of PM10 for one station, as `GET /temporal/entities/{id}` answers it. */
export function history(id: string, now: Date, points = 4): Record<string, unknown> {
  return {
    id,
    type: "AirQualityObserved",
    pm10: Array.from({ length: points }, (_, index) => ({
      type: "Property",
      value: 10 + index,
      unitCode: "GQ",
      observedAt: new Date(now.getTime() - (points - index) * 3_600_000).toISOString(),
    })),
  };
}

/**
 * Station SK0263A exactly as the pipelines ovzdusie-pm10 and ovzdusie-pm25 write it
 * (`pipeline-ovzdusie-pm10-bento.yaml`): one entity, each stream its own attribute, the place and
 * the name from the EEA station metadata, the reading `minutesAgo` before `now`.
 */
export function sk0263a(now: Date, minutesAgo: number, pm10 = 21.3, pm25 = 14.8): Record<string, unknown> {
  const at = new Date(now.getTime() - minutesAgo * 60_000).toISOString();
  return {
    id: `${URN}:eea-SK0263A`,
    type: "AirQualityObserved",
    name: {
      type: "LanguageProperty",
      languageMap: { sk: "Stanica SK0263A, mestské pozadie", en: "Station SK0263A, urban background" },
    },
    stationCode: { type: "Property", value: "SK0263A" },
    pm10: { type: "Property", value: pm10, unitCode: "GQ", observedAt: at },
    pm25: { type: "Property", value: pm25, unitCode: "GQ", observedAt: at },
    dateObserved: { type: "Property", value: at },
    location: { type: "GeoProperty", value: { type: "Point", coordinates: [19.115268, 48.733256] } },
    dataProvider: { type: "Property", value: "European Environment Agency, air quality data reported by SHMÚ (CC BY 4.0)" },
    source: { type: "Property", value: "https://eeadmz1-downloads-api-appservice.azurewebsites.net/" },
  };
}
