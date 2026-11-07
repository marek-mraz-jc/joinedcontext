import type { Row } from "@joinedcontext/sdk";

const station = (local: string, name: string, bikes: number | undefined, free: number, lon: number, lat: number, status = "working"): Row => ({
  id: `urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:${local}`,
  type: "BikeHireDockingStation",
  name,
  ...(bikes === undefined ? {} : { availableBikeNumber: bikes, totalSlotNumber: bikes + free }),
  freeSlotNumber: free,
  status,
  dateModified: "2030-10-20T05:55:00Z",
  location: { type: "Point", coordinates: [lon, lat] },
});

/**
 * Eight stations as the bikes pipeline writes them (T-3328): two full, two empty, one low, one
 * balanced, one that sends no bike count and one out of service.
 */
export const STATIONS: Row[] = [
  station("001", "Kaivopuisto", 28, 2, 24.9502, 60.1557),
  station("002", "Laivasillankatu", 0, 12, 24.9564, 60.1605),
  station("003", "Kapteeninpuistikko", 16, 0, 24.9444, 60.1583),
  station("004", "Viiskulma", undefined, 14, 24.9412, 60.1604),
  station("005", "Sepänkatu", 0, 20, 24.9362, 60.1602),
  station("006", "Hietalahdentori", 2, 18, 24.9295, 60.1621),
  station("007", "Designmuseo", 10, 10, 24.9459, 60.1631),
  station("008", "Rautatientori", 30, 0, 24.9414, 60.1710, "outOfService"),
];
