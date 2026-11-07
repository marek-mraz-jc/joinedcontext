/**
 * What the public endpoint of `banskabystrica-verejne` answers, as the gateway shapes it and the
 * pipelines podujatia, skoly and ovzdusie-pm10 write it (T-2781, T-2949): names as LanguageProperty,
 * positions as GeoProperty, a missing value missing.
 */
const URN = (type: string, id: string) => `urn:ngsi-ld:${type}:banskabystrica.sk:banskabystrica-verejne:${id}`;

const point = (lon: number, lat: number) => ({ type: "GeoProperty", value: { type: "Point", coordinates: [lon, lat] } });
const name = (sk: string) => ({ type: "LanguageProperty", languageMap: { sk } });
const value = (v: unknown) => ({ type: "Property", value: v });

export const EVENTS = [
  {
    id: URN("Event", "jarmok-2026"),
    type: "Event",
    name: name("Radvanský jarmok"),
    startDate: value("2026-10-08"),
    endDate: value("2026-10-10"),
    startTime: value("09:00:00"),
    eventCategory: value("other"),
    address: value("Námestie SNP, Banská Bystrica"),
    url: value("https://www.banskabystrica.sk/podujatia/radvansky-jarmok"),
    location: point(19.1459, 48.7357),
  },
  {
    id: URN("Event", "koncert-2026-09"),
    type: "Event",
    name: name("Koncert v parku"),
    startDate: value("2026-09-12"),
    eventCategory: value("musicDanceTheatre"),
    address: value("Mestský park, Banská Bystrica"),
    url: value("javascript:alert(1)"),
    location: point(19.15, 48.739),
  },
  {
    id: URN("Event", "vystava-bez-polohy"),
    type: "Event",
    name: name("Výstava fotografií"),
    startDate: value("2026-10-20"),
    eventCategory: value("exhibition"),
  },
];

export const SCHOOLS = [
  {
    id: URN("Organization", "zs-moyzesova"),
    type: "Organization",
    name: name("Základná škola, Moyzesova 18"),
    address: value("Moyzesova 18, Banská Bystrica"),
    pupilCount: value(512),
    teachingLanguage: value("slovenský"),
    location: point(19.1531, 48.7392),
  },
  {
    id: URN("Organization", "gymnazium-tajovskeho"),
    type: "Organization",
    name: name("Gymnázium Jozefa Gregora Tajovského"),
    address: value("Tajovského 25, Banská Bystrica"),
    location: point(19.1302, 48.7352),
  },
];

export const AIR = [
  {
    id: URN("AirQualityObserved", "SK0263A"),
    type: "AirQualityObserved",
    name: name("Banská Bystrica, Štefánikovo nábrežie"),
    stationCode: value("SK0263A"),
    dateObserved: value("2026-10-06T08:00:00Z"),
    pm10: { type: "Property", value: 21.4, unitCode: "GQ" },
    location: point(19.1546, 48.7337),
  },
];

/** The rows of one type, as `GET …/entities?type=…` answers them. */
export function answer(type: string | null): unknown[] {
  if (type === "Event") return EVENTS;
  if (type === "Organization") return SCHOOLS;
  if (type === "AirQualityObserved") return AIR;
  return [];
}
