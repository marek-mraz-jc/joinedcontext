import type { Row } from "@joinedcontext/sdk";

const station = (local: string, name: string, bikesNow: number | undefined, free: number, lon: number, lat: number): Row => ({
  id: `urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:${local}`,
  type: "BikeHireDockingStation",
  name,
  ...(bikesNow === undefined ? {} : { availableBikeNumber: bikesNow }),
  freeSlotNumber: free,
  totalSlotNumber: (bikesNow ?? 0) + free,
  status: "working",
  dateModified: "2026-09-22T06:00:00Z",
  location: { type: "Point", coordinates: [lon, lat] },
});

/** Five stations sampled from the city's feed: two empty-ish cases, one that sends no count. */
export const STATIONS: Row[] = [
  station("001", "Kaivopuisto", 4, 26, 24.9502, 60.1557),
  station("002", "Laivasillankatu", 0, 12, 24.9564, 60.1605),
  station("003", "Kapteeninpuistikko", 11, 5, 24.9444, 60.1583),
  station("004", "Viiskulma", undefined, 14, 24.9412, 60.1604),
  station("005", "Sepänkatu", 0, 20, 24.9362, 60.1602),
];
