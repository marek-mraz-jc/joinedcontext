/**
 * Types and transformations for district comparison: converting SDK rows into WASM input,
 * extracting district names, values, default selections, and serializing/deserializing view state.
 */
import type { Row } from "@joinedcontext/sdk";
import { format, pointOf } from "@joinedcontext/sdk";

export type Measure = "events" | "bikes" | "bikeSlots" | "alerts" | "pm25" | "aqi";

export const MEASURES: readonly Measure[] = ["events", "bikes", "bikeSlots", "alerts", "pm25", "aqi"] as const;

export type PerKm2Measure = "events" | "bikes" | "bikeSlots" | "alerts";

export interface DistrictInput {
  code: string;
  name: string;
  geometry: unknown;
}

export interface BikeInput {
  at: [number, number];
  slots: number | null;
}

export interface AirInput {
  at: [number, number];
  pm25: number | null;
  aqi: number | null;
}

export interface CompareInput {
  districts: DistrictInput[];
  events: [number, number][];
  bikes: BikeInput[];
  alerts: [number, number][];
  air: AirInput[];
}

export interface DistrictOutput {
  code: string;
  name: string;
  areaKm2: number;
  events: number;
  bikes: number;
  bikeSlots: number;
  alerts: number;
  pm25: number | null;
  aqi: number | null;
  perKm2: {
    events: number | null;
    bikes: number | null;
    bikeSlots: number | null;
    alerts: number | null;
  };
  rank: Record<Measure, number | null>;
}

export interface CompareOutput {
  districts: DistrictOutput[];
  outside: {
    events: number;
    bikes: number;
    alerts: number;
    air: number;
  };
}

export interface RowsByType {
  districts: Row[];
  events: Row[];
  bikes: Row[];
  alerts: Row[];
  air: Row[];
}

/**
 * What a district is called: its `name`, which the SDK has already resolved from the entity's
 * language map (fi, sv) to one string, else its code.
 */
export function districtName(row: Row): string {
  const name = format(row.name).trim();
  // A URN is never what a person reads (the SDK's displayName holds the same rule).
  return (name.startsWith("urn:") ? "" : name) || String(row.districtCode ?? "") || row.id;
}

/** Converts SDK rows into the shape expected by the Rust/WASM comparison module. */
export function toInput(rows: RowsByType): CompareInput {
  const { districts: districtRows, events: eRows, bikes: bRows, alerts: alRows, air: aiRows } = rows;

  const districts: DistrictInput[] = [];
  for (const r of districtRows) {
    if (r.divisionLevel !== "district") continue;
    const code = String(r.districtCode ?? "");
    const name = districtName(r);
    districts.push({
      code,
      name,
      geometry: r.location ?? null,
    });
  }

  // Points without valid coordinates are represented with out-of-range coordinates to be counted in `outside`.
  const events: [number, number][] = eRows.map((r) => {
    const p = pointOf(r.location);
    return p ? [p[0], p[1]] : [999.0, 999.0];
  });

  const bikes: BikeInput[] = bRows.map((r) => {
    const p = pointOf(r.location);
    const slots =
      typeof r.totalSlotNumber === "number"
        ? r.totalSlotNumber
        : typeof r.totalSlotNumber === "string" && !Number.isNaN(Number(r.totalSlotNumber))
          ? Number(r.totalSlotNumber)
          : null;
    return {
      at: p ? [p[0], p[1]] : [999.0, 999.0],
      slots,
    };
  });

  const alerts: [number, number][] = alRows.map((r) => {
    const p = pointOf(r.location);
    return p ? [p[0], p[1]] : [999.0, 999.0];
  });

  const air: AirInput[] = aiRows.map((r) => {
    const p = pointOf(r.location);
    const pm25 =
      typeof r.pm25 === "number"
        ? r.pm25
        : typeof r.pm25 === "string" && !Number.isNaN(Number(r.pm25))
          ? Number(r.pm25)
          : null;
    const aqi =
      typeof r.airQualityIndex === "number"
        ? r.airQualityIndex
        : typeof r.airQualityIndex === "string" && !Number.isNaN(Number(r.airQualityIndex))
          ? Number(r.airQualityIndex)
          : null;
    return {
      at: p ? [p[0], p[1]] : [999.0, 999.0],
      pm25,
      aqi,
    };
  });

  return { districts, events, bikes, alerts, air };
}

/** Whether the given measure is normalized per km² (air quality metrics are not). */
export function hasPerKm2(measure: Measure): boolean {
  return measure === "events" || measure === "bikes" || measure === "bikeSlots" || measure === "alerts";
}

/** Returns the raw value of a measure for a district. */
export function measureValue(district: DistrictOutput, measure: Measure): number | null {
  switch (measure) {
    case "events":
      return district.events;
    case "bikes":
      return district.bikes;
    case "bikeSlots":
      return district.bikeSlots;
    case "alerts":
      return district.alerts;
    case "pm25":
      return district.pm25;
    case "aqi":
      return district.aqi;
  }
}

/** Returns the per km² value for a measure, or null if not applicable. */
export function measurePerKm2(district: DistrictOutput, measure: Measure): number | null {
  if (!hasPerKm2(measure)) return null;
  return district.perKm2[measure as PerKm2Measure];
}

/** The value used for the choropleth map: density per km² for counts, mean value for air quality. */
export function choroplethValue(district: DistrictOutput, measure: Measure): number | null {
  if (hasPerKm2(measure)) {
    return district.perKm2[measure as PerKm2Measure];
  }
  return measureValue(district, measure);
}

/** The two districts with the most events, preselected on arrival; fewer districts select all. */
export function defaultSelection(output: CompareOutput | null): string[] {
  if (!output || output.districts.length === 0) return [];
  if (output.districts.length <= 2) {
    return output.districts.map((d) => d.code);
  }
  const sorted = [...output.districts].sort((a, b) => {
    if (b.events !== a.events) return b.events - a.events;
    return a.name.localeCompare(b.name);
  });
  return [sorted[0].code, sorted[1].code];
}

export interface HashState {
  selectedCodes: string[];
  measure: Measure;
}

/** Parses the hash state `#compare?d=101,102&m=events`. */
export function readHash(hash: string): HashState {
  const qIdx = hash.indexOf("?");
  const search = qIdx >= 0 ? hash.slice(qIdx + 1) : "";
  const params = new URLSearchParams(search);
  const rawCodes = params.get("d");
  const selectedCodes = rawCodes
    ? rawCodes
        .split(",")
        .map((c) => c.trim())
        .filter(Boolean)
    : [];
  const rawM = params.get("m");
  const measure: Measure =
    rawM && (MEASURES as readonly string[]).includes(rawM)
      ? (rawM as Measure)
      : "events";
  return { selectedCodes, measure };
}

/** Formats the hash state `#compare?d=101,102&m=events`. */
export function writeHash(state: HashState, page = "compare"): string {
  const params = new URLSearchParams();
  if (state.selectedCodes.length > 0) {
    params.set("d", state.selectedCodes.join(","));
  }
  if (state.measure) {
    params.set("m", state.measure);
  }
  const query = params.toString();
  return `#${page}${query ? `?${query}` : ""}`;
}

export const readView = readHash;
export const writeView = writeHash;

/** Sorts districts by competition rank for the selected measure, null ranks at the end. */
export function sortDistrictsByMeasure(districts: DistrictOutput[], measure: Measure): DistrictOutput[] {
  return [...districts].sort((a, b) => {
    const ra = a.rank[measure];
    const rb = b.rank[measure];
    if (ra !== null && rb !== null) {
      if (ra !== rb) return ra - rb;
      return a.name.localeCompare(b.name);
    }
    if (ra !== null) return -1;
    if (rb !== null) return 1;
    return a.name.localeCompare(b.name);
  });
}
