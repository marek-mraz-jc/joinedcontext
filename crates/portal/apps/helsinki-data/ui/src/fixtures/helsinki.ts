/**
 * What the public endpoint of `helsinki` answers, as the pipelines services and beach-water write it
 * (T-2787): names as LanguageProperty in Finnish, Swedish and English where the register has them,
 * positions as GeoProperty, the category as `serviceCategory`, the sensor's place a Relationship.
 */
const URN = (type: string, id: string) => `urn:ngsi-ld:${type}:hel.fi:helsinki:${id}`;
const point = (lon: number, lat: number) => ({ type: "GeoProperty", value: { type: "Point", coordinates: [lon, lat] } });
const value = (v: unknown) => ({ type: "Property", value: v });

export const SERVICES = [
  { id: URN("PointOfInterest", "oodi"), type: "PointOfInterest", name: { type: "LanguageProperty", languageMap: { fi: "Keskustakirjasto Oodi", sv: "Centrumbiblioteket Ode", en: "Central Library Oodi" } }, serviceCategory: value("library"), address: value("Töölönlahdenkatu 4"), openingHours: value("ma–pe 8–22, la–su 10–20"), url: value("https://www.oodihelsinki.fi/"), location: point(24.9381, 60.1739) },
  { id: URN("PointOfInterest", "kallio-ta"), type: "PointOfInterest", name: { type: "LanguageProperty", languageMap: { fi: "Kallion terveysasema" } }, serviceCategory: value("healthStation"), address: value("Sturenkatu 18"), location: point(24.9496, 60.1873) },
  { id: URN("PointOfInterest", "yrjonkatu"), type: "PointOfInterest", name: { type: "LanguageProperty", languageMap: { fi: "Yrjönkadun uimahalli" } }, serviceCategory: value("swimmingHall"), address: value("Yrjönkatu 21b"), location: point(24.9376, 60.1659) },
  { id: URN("PointOfInterest", "hietaniemi"), type: "PointOfInterest", name: { type: "LanguageProperty", languageMap: { fi: "Hietaniemen uimaranta" } }, serviceCategory: value("beach"), location: point(24.9123, 60.1713) },
  { id: URN("PointOfInterest", "kallion-koulu"), type: "PointOfInterest", name: { type: "LanguageProperty", languageMap: { fi: "Kallion ala-asteen koulu" } }, serviceCategory: value("school"), url: value("javascript:alert(1)") },
  { id: URN("PointOfInterest", "muu"), type: "PointOfInterest", name: { type: "LanguageProperty", languageMap: { fi: "Muu palvelu" } }, serviceCategory: value("other"), location: point(24.94, 60.17) },
];

export const WATER = [
  { id: URN("WaterQualityObserved", "hietaniemi-anturi"), type: "WaterQualityObserved", name: { type: "LanguageProperty", languageMap: { fi: "Hietaniemi, vesi" } }, temperature: value(16.4), dateObserved: value("2026-08-15T09:00:00Z"), refPointOfInterest: { type: "Relationship", object: URN("PointOfInterest", "hietaniemi") }, location: point(24.9119, 60.1709) },
  { id: URN("WaterQualityObserved", "irrallinen"), type: "WaterQualityObserved", name: { type: "LanguageProperty", languageMap: { fi: "Irrallinen anturi" } }, location: point(24.95, 60.15) },
];

export const PERMITS = [
  { id: URN("CityWork", "kaivuu-1"), type: "CityWork", name: { type: "LanguageProperty", languageMap: { fi: "Kaukolämpöputken korjaus" } }, permitKind: value("excavation"), permitStatus: value("ongoing"), permitNumber: value("KAI-2026-1234"), permitStart: value("2026-10-01"), permitEnd: value("2026-10-20"), address: value("Mannerheimintie 5") },
];

const fi = (text: string) => ({ type: "LanguageProperty", languageMap: { fi: text } });

/** One of each other type the grids show, as their pipelines write them (T-3396). */
export const OTHERS: Record<string, unknown[]> = {
  Event: [{ id: URN("Event", "helsinki-paivat"), type: "Event", name: fi("Helsinki-päivä"), startDate: value("2030-06-12T10:00:00+03:00"), endDate: value("2030-06-12T22:00:00+03:00"), eventStatus: value("EventScheduled"), address: value("Kaivopuisto") }],
  Alert: [{ id: URN("Alert", "GUID1"), type: "Alert", name: fi("Mannerheimintie, Helsinki. Tietyö."), category: value("traffic"), subCategory: value("ROAD_WORK"), address: value("Helsinki"), dateIssued: value("2030-10-01T08:00:00Z"), validFrom: value("2030-10-07T05:00:00Z"), validTo: value("2030-10-21T15:00:00Z") }],
  OnStreetParking: [{ id: URN("OnStreetParking", "fee-zone-1"), type: "OnStreetParking", name: fi("Maksuvyöhyke 1"), category: value(["feeCharged"]), zoneCode: value("1"), url: value("https://www.hel.fi/pysakointi") }],
  AdministrativeArea: [{ id: URN("AdministrativeArea", "district-1"), type: "AdministrativeArea", name: fi("Kruununhaka"), districtCode: value("1"), divisionLevel: value("district") }],
  AirQualityObserved: [{ id: URN("AirQualityObserved", "kallio"), type: "AirQualityObserved", name: fi("Kallio 2"), dateObserved: value("2030-10-07T09:00:00Z"), pm10: value(12.5), pm25: value(5.1), airQualityIndex: value(2) }],
  WeatherObserved: [{ id: URN("WeatherObserved", "lahdenvayla"), type: "WeatherObserved", name: fi("vt4 Lahdenväylä"), dateObserved: value("2030-10-07T09:00:00Z"), temperature: value(6.2), roadSurfaceTemperature: value(4.8), relativeHumidity: value(88), windSpeed: value(3.4), windDirection: value(210), precipitation: value(0.2) }],
};

/** The rows of one type, as `GET …/entities?type=…` answers them. */
export function answer(type: string | null): unknown[] {
  if (type === "PointOfInterest") return SERVICES;
  if (type === "WaterQualityObserved") return WATER;
  if (type === "CityWork") return PERMITS;
  return OTHERS[type ?? ""] ?? [];
}

/** One entity by its id, as `GET …/entities/{id}` answers it (the entity panel's read), or none. */
export function byId(id: string): unknown | undefined {
  return [...SERVICES, ...WATER, ...PERMITS, ...Object.values(OTHERS).flat()].find((entity) => (entity as { id: string }).id === id);
}
