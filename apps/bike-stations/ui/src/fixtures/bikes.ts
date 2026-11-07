/**
 * What the helsinki project's public `helsinki-bikes` endpoint answers, as the citybikes-gbfs
 * pipeline writes it: the name a Property, the counts integers, the position a GeoProperty. One
 * station is empty, one full, one reports no counts and one has no position.
 */
const URN = (id: string) => `urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:${id}`;
const point = (lon: number, lat: number) => ({ type: "GeoProperty", value: { type: "Point", coordinates: [lon, lat] } });
const value = (v: unknown) => ({ type: "Property", value: v });

export const STATIONS = [
  { id: URN("001"), type: "BikeHireDockingStation", name: value("Kaivopuisto"), availableBikeNumber: value(7), freeSlotNumber: value(23), totalSlotNumber: value(30), status: value("working"), dateModified: value("2026-10-07T08:55:00Z"), location: point(24.9502, 60.1557) },
  { id: URN("002"), type: "BikeHireDockingStation", name: value("Töölönlahdenkatu"), availableBikeNumber: value(0), freeSlotNumber: value(16), totalSlotNumber: value(16), status: value("working"), location: point(24.9381, 60.1739) },
  { id: URN("003"), type: "BikeHireDockingStation", name: value("Rautatientori"), availableBikeNumber: value(24), freeSlotNumber: value(0), totalSlotNumber: value(24), status: value("working"), location: point(24.9433, 60.1712) },
  { id: URN("004"), type: "BikeHireDockingStation", name: value("Sompasaari"), status: value("closed"), location: point(24.9757, 60.1806) },
  { id: URN("005"), type: "BikeHireDockingStation", name: value("Kalasatama (metro)"), availableBikeNumber: value(3), freeSlotNumber: value(9), totalSlotNumber: value(12) },
];

/** The rows of one type, as `GET …/entities?type=…` answers them. */
export function answer(type: string | null): unknown[] {
  return type === "BikeHireDockingStation" ? STATIONS : [];
}
