import { format, pointOf } from "@joinedcontext/sdk";
import type { DesignTokens, Row } from "@joinedcontext/sdk";
import type { Level, Station } from "./planner";

/** The one entity type the app's data need names (AP-04). */
export const STATION = "BikeHireDockingStation";

/** The attributes the page reads, exactly those app.yaml names. */
export const ATTRS = ["name", "location", "availableBikeNumber", "freeSlotNumber", "totalSlotNumber", "status", "dateModified"];

/** A whole count of bikes or places, or `null`: a missing or broken count is never read as zero. */
export function countOf(value: unknown): number | null {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : null;
}

/** Whether HSL rents and takes back bikes there; a station out of service is never routed. */
export function inService(row: Row): boolean {
  const status = format(row.status).trim();
  return status === "" || status === "working";
}

/** The station's name, else the end of its id. */
export function nameOf(row: Row): string {
  return format(row.name).trim() || row.id.slice(row.id.lastIndexOf(":") + 1);
}

/** The rows as the planner reads them; a station out of service keeps its counts out of the plan. */
export function stationsOf(rows: Row[]): Station[] {
  return rows.map((row) => {
    const at = pointOf(row.location);
    const working = inService(row);
    return {
      id: row.id,
      name: nameOf(row),
      lon: at ? at[0] : null,
      lat: at ? at[1] : null,
      bikes: working ? countOf(row.availableBikeNumber) : null,
      free: working ? countOf(row.freeSlotNumber) : null,
      capacity: working ? countOf(row.totalSlotNumber) : null,
    };
  });
}

/** The colour of a level on the map and in the table: the design tokens' own, never a new palette. */
export function levelColour(level: Level, tokens: DesignTokens): string {
  switch (level) {
    case "empty":
      return tokens.color.danger;
    case "low":
      return tokens.color.warning;
    case "high":
      return tokens.map.high;
    case "full":
      return tokens.color.accent;
    case "balanced":
      return tokens.color.success;
    default:
      return tokens.color.muted;
  }
}

/** The newest `dateModified` of the rows: when the counts on screen were read by the city's feed. */
export function newest(rows: Row[]): Date | null {
  let found: Date | null = null;
  for (const row of rows) {
    const value = typeof row.dateModified === "string" ? new Date(row.dateModified) : null;
    if (value && !Number.isNaN(value.getTime()) && (!found || value > found)) found = value;
  }
  return found;
}
