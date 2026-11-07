/**
 * What the public endpoint of `praha-mesto` answers, as the pipelines schools, culture, toilets,
 * ticket-points and waste-stations write it (T-2785): names as LanguageProperty, positions as
 * GeoProperty, the category as `serviceCategory`, a value the source did not publish missing.
 */
const URN = (type: string, id: string) => `urn:ngsi-ld:${type}:praha.eu:praha-mesto:${id}`;
const point = (lon: number, lat: number) => ({ type: "GeoProperty", value: { type: "Point", coordinates: [lon, lat] } });
const name = (cs: string) => ({ type: "LanguageProperty", languageMap: { cs } });
const value = (v: unknown) => ({ type: "Property", value: v });

export const POIS = [
  { id: URN("PointOfInterest", "zs-vodickova"), type: "PointOfInterest", name: name("Základní škola Vodičkova"), serviceCategory: value("school"), facilityType: value("základní škola"), pupilCount: value(410), address: value({ streetAddress: "Vodičkova 22, Praha 1", addressLocality: "Praha", addressCountry: "CZ" }), location: point(14.4232, 50.0807) },
  { id: URN("PointOfInterest", "narodni-divadlo"), type: "PointOfInterest", name: name("Národní divadlo"), serviceCategory: value("culture"), facilityType: value("divadlo"), capacity: value(986), url: value("https://www.narodni-divadlo.cz/"), address: value({ streetAddress: "Národní 2, Praha 1", addressLocality: "Praha", addressCountry: "CZ" }), location: point(14.4134, 50.0812) },
  { id: URN("PointOfInterest", "wc-andel"), type: "PointOfInterest", name: name("Veřejné WC Anděl"), serviceCategory: value("publicToilet"), openingHours: value("6:00–22:00"), wheelchairAccessible: value(true), address: value({ streetAddress: "Nádražní, Praha 5", addressLocality: "Praha", addressCountry: "CZ" }), location: point(14.4036, 50.0703) },
  { id: URN("PointOfInterest", "jizdenky-muzeum"), type: "PointOfInterest", name: name("Předprodej jízdenek Muzeum"), serviceCategory: value("ticketSale"), wheelchairAccessible: value(false), location: point(14.4306, 50.0794) },
  { id: URN("PointOfInterest", "bez-kategorie"), type: "PointOfInterest", name: name("Místo bez kategorie"), location: point(14.42, 50.08) },
  { id: URN("PointOfInterest", "skript"), type: "PointOfInterest", name: name("Galerie se skriptem"), serviceCategory: value("culture"), url: value("javascript:alert(1)") },
];

export const ISLES = [
  { id: URN("WasteContainerIsle", "0001-001"), type: "WasteContainerIsle", name: name("Vinohradská 12"), stationCode: value("0001/ 001"), accessRestriction: value("public"), location: point(14.4401, 50.0771) },
  { id: URN("WasteContainerIsle", "0002-004"), type: "WasteContainerIsle", name: name("Korunní 40"), stationCode: value("0002/ 004"), accessRestriction: value("residents"), location: point(14.4459, 50.0752) },
];

export const CONTAINERS = [
  { id: URN("WasteContainer", "c-1"), type: "WasteContainer", containerCode: value("0001-001-PAP"), wasteKind: value("paper"), fillingLevel: value(0.82), dateModified: value("2026-10-06T07:40:00Z") },
  { id: URN("WasteContainer", "c-2"), type: "WasteContainer", containerCode: value("0001-001-PLA"), wasteKind: value("plastic"), fillingLevel: value(0.35), dateModified: value("2026-10-06T07:40:00Z") },
];

export const BUDGET = [
  { id: URN("BudgetLine", "2026-0001"), type: "BudgetLine", budgetItem: value("Údržba komunikací"), fiscalYear: value(2026), budgetArea: value("Doprava"), approvedAmount: value(1250000000), adjustedAmount: value(1310000000) },
];

/** One of each other type the grids show, as their pipelines write them (T-3402). */
export const OTHERS: Record<string, unknown[]> = {
  OffStreetParking: [{ id: URN("OffStreetParking", "pr-zlicin"), type: "OffStreetParking", name: name("P+R Zličín"), totalSpotNumber: value(470), availableSpotNumber: value(112), occupiedSpotNumber: value(358), dateModified: value("2026-10-06T07:40:00Z") }],
  BikeHireDockingStation: [{ id: URN("BikeHireDockingStation", "nextbike-1"), type: "BikeHireDockingStation", name: name("Václavské náměstí"), totalSlotNumber: value(20), availableBikeNumber: value(7), freeSlotNumber: value(13), status: value("working"), dateModified: value("2026-10-06T07:40:00Z") }],
  AirQualityObserved: [{ id: URN("AirQualityObserved", "legerova"), type: "AirQualityObserved", name: name("Praha 2 – Legerova"), dateObserved: value("2026-10-06T07:00:00Z"), pm10: value(21), pm25: value(12), no2: value(38), o3: value(41), so2: value(3), co: value(0.4) }],
  AdministrativeArea: [{ id: URN("AdministrativeArea", "praha-1"), type: "AdministrativeArea", name: name("Praha 1"), districtCode: value("500054") }],
};

/** The rows of one type, as `GET …/entities?type=…` answers them. */
export function answer(type: string | null): unknown[] {
  if (type === "PointOfInterest") return POIS;
  if (type === "WasteContainerIsle") return ISLES;
  if (type === "WasteContainer") return CONTAINERS;
  if (type === "BudgetLine") return BUDGET;
  return OTHERS[type ?? ""] ?? [];
}

/** One entity by its id, as `GET …/entities/{id}` answers it (the entity panel's read), or none. */
export function byId(id: string): unknown | undefined {
  return [...POIS, ...ISLES, ...CONTAINERS, ...BUDGET, ...Object.values(OTHERS).flat()].find((entity) => (entity as { id: string }).id === id);
}
