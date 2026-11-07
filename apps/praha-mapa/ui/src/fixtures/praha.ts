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

/** The rows of one type, as `GET …/entities?type=…` answers them. */
export function answer(type: string | null): unknown[] {
  if (type === "PointOfInterest") return POIS;
  if (type === "WasteContainerIsle") return ISLES;
  return [];
}
