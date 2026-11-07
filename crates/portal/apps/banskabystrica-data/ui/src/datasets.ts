/**
 * The city's public datasets as grids (T-2782): one grid per type of `banskabystrica-verejne`, the
 * columns the model gives it with the city's own labels, the name pinned as the row's primary
 * field, and every column filterable at the endpoint.
 */
import { parseGridConfig } from "@joinedcontext/sdk";
import type { ResolvedGridConfig } from "@joinedcontext/sdk";

export const DATASETS = ["events", "schools", "air"] as const;
export type Dataset = (typeof DATASETS)[number];

export const TYPE_OF: Record<Dataset, string> = { events: "Event", schools: "Organization", air: "AirQualityObserved" };

/** The columns of each grid, in the order a reader reads them, the primary field first. */
export const COLUMNS: Record<Dataset, string[]> = {
  events: ["name", "startDate", "endDate", "startTime", "eventCategory", "address", "url"],
  schools: ["name", "schoolCode", "address", "teachingLanguage", "pupilCount", "teachingStaff", "nonTeachingStaff", "annualBudget", "budgetYear"],
  air: ["name", "stationCode", "dateObserved", "pm10", "pm25"],
};

/** The model's enum `EventCategory`, the one column that is picked rather than typed. */
export const EVENT_CATEGORIES = ["musicDanceTheatre", "museumsGalleriesLibraries", "sport", "exhibition", "other"] as const;

export function gridConfig(slug: string, dataset: Dataset, labels: Record<string, string>): ResolvedGridConfig {
  const parsed = parseGridConfig({
    source: { kind: "endpoint", slug },
    type: TYPE_OF[dataset],
    columns: COLUMNS[dataset].map((attr) => ({ attr, label: labels[attr] ?? attr, ...(attr === "name" ? { pinned: true } : {}) })),
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
