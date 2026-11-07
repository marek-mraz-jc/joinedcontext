/**
 * Prague's open data as grids (T-2786): one grid per type of `praha-mesto`, the columns the model
 * gives it with the city's own labels, the first column pinned as the row's primary field (the
 * name, or the code where the type has no name), and every column filterable at the endpoint.
 */
import { parseGridConfig } from "@joinedcontext/sdk";
import type { ResolvedGridConfig } from "@joinedcontext/sdk";

export const DATASETS = ["places", "isles", "containers", "parking", "bikes", "air", "districts", "budget"] as const;
export type Dataset = (typeof DATASETS)[number];

export const TYPE_OF: Record<Dataset, string> = {
  places: "PointOfInterest",
  isles: "WasteContainerIsle",
  containers: "WasteContainer",
  parking: "OffStreetParking",
  bikes: "BikeHireDockingStation",
  air: "AirQualityObserved",
  districts: "AdministrativeArea",
  budget: "BudgetLine",
};

/** The columns of each grid, in the order a reader reads them, the primary field first. */
export const COLUMNS: Record<Dataset, string[]> = {
  places: ["name", "serviceCategory", "facilityType", "address", "openingHours", "wheelchairAccessible", "capacity", "pupilCount", "url"],
  isles: ["name", "stationCode", "accessRestriction"],
  containers: ["containerCode", "wasteKind", "fillingLevel", "dateModified"],
  parking: ["name", "totalSpotNumber", "availableSpotNumber", "occupiedSpotNumber", "dateModified"],
  bikes: ["name", "totalSlotNumber", "availableBikeNumber", "freeSlotNumber", "status", "dateModified"],
  air: ["name", "dateObserved", "pm10", "pm25", "no2", "o3", "so2", "co"],
  districts: ["name", "districtCode"],
  budget: ["budgetItem", "fiscalYear", "budgetArea", "budgetFunction", "budgetPurpose", "approvedAmount", "adjustedAmount"],
};

/** The model's enums, by attribute: the columns that are picked rather than typed. */
export const ENUMS: Record<string, readonly string[]> = {
  serviceCategory: ["school", "culture", "publicToilet", "ticketSale"],
  accessRestriction: ["public", "residents"],
  wasteKind: ["colouredGlass", "clearGlass", "paper", "plastic", "metal", "beverageCartons", "electronics", "edibleOil", "mixedRecyclables"],
  status: ["working", "outOfService"],
};

export function gridConfig(slug: string, dataset: Dataset, labels: Record<string, string>): ResolvedGridConfig {
  const [primary] = COLUMNS[dataset];
  const parsed = parseGridConfig({
    source: { kind: "endpoint", slug },
    type: TYPE_OF[dataset],
    columns: COLUMNS[dataset].map((attr) => ({ attr, label: labels[attr] ?? attr, ...(attr === primary ? { pinned: true } : {}) })),
    // Every column is one the endpoint answers a `q` about, so a filter is the endpoint's own.
    filters: { allowed: [...COLUMNS[dataset]] },
    pageSize: 25,
    mode: "view",
  });
  if (!parsed.config) {
    throw new Error(`grid ${dataset}: ${parsed.findings.map((f) => f.message).join("; ")}`);
  }
  return parsed.config;
}

/**
 * The endpoint's own download of a whole dataset (API/02 §3, §4): the same grant as the grid, so
 * the file holds nothing the grid would not show. CSV with human headers;
 * the gateway serves no GeoJSON file since T-4597.
 */
export function exportUrl(slug: string, dataset: Dataset): string {
  const query = new URLSearchParams({ type: TYPE_OF[dataset] });
  query.set("humanHeaders", "true");
  return `/api/endpoint/${encodeURIComponent(slug)}/file.csv?${query.toString()}`;
}
