import type { Row } from "@joinedcontext/sdk";

const station = (local: string, name: string, bikes: number | undefined, free: number, lon: number, lat: number, open = true): Row => ({
  id: `urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:${local}`,
  type: "BikeHireDockingStation",
  name,
  ...(bikes === undefined ? {} : { availableBikeNumber: bikes }),
  freeSlotNumber: free,
  status: open ? "working" : "outOfService",
  location: { type: "Point", coordinates: [lon, lat] },
});

/**
 * Stations as the endpoint answers them, for the tests and the gallery's browser check: one
 * empty, one that sends no count, one out of service.
 */
export const ROWS: Row[] = [
  station("001", "Kaivopuisto", 4, 26, 24.9502, 60.1557),
  station("002", "Kamppi", 0, 12, 24.9316, 60.1690),
  station("003", "Kallio", undefined, 20, 24.9496, 60.1841, false),
  station("004", "Hakaniemi", 11, 9, 24.9510, 60.1790),
  station("005", "Rautatientori", 17, 3, 24.9440, 60.1710),
  station("006", "Töölönlahti", 6, 14, 24.9380, 60.1780),
  station("007", "Ruoholahti", 2, 18, 24.9150, 60.1630),
  station("008", "Sörnäinen", 9, 11, 24.9620, 60.1870),
];
