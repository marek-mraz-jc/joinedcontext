/**
 * The rows of a page as shapes on the grid's map (UI-72).
 *
 * The map is a second view of the same rows, so everything it shows is derived from them and
 * nothing is queried twice. These are the derivations, apart from the component so they can be
 * tested without a WebGL canvas: which attribute the map is about, which rows have a geometry, and
 * how far a geometry moved once a person edits one.
 */

import type { GeoFeature } from "../geo/GeoView";
import type { Geometry, GeometryType, Position } from "../geo/validate";
import type { RichCell, RichRow } from "./model";

/** The first cell of an attribute, which is the one the map draws for a multi-instance attribute. */
export function firstCell(row: RichRow, attr: string): RichCell | undefined {
  const cell = row.cells[attr];
  return Array.isArray(cell) ? cell[0] : cell;
}

/** The geometry of one row, or `null` for a row whose cell is missing or is not a geometry. */
export function geometryOfRow(row: RichRow, attr: string): Geometry | null {
  const cell = firstCell(row, attr);
  if (!cell || cell.kind !== "geo") {
    return null;
  }
  const value = cell.value;
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const geometry = value as { type?: unknown; coordinates?: unknown };
  return typeof geometry.type === "string" && geometry.coordinates !== undefined ? (value as Geometry) : null;
}

/**
 * Which attribute the map is about: the one the config names, else the first attribute of the page
 * that holds a geometry. `null` means this type has none, and then there is no map at all — the
 * host offers no panel and no "show on map" action, rather than an empty canvas (UI-72).
 */
export function mapAttrOf(rows: RichRow[], named?: string): string | null {
  if (named) {
    // A named attribute that no row carries still owns the panel: the page may be filtered down to
    // rows without it, and the map must not silently move to another attribute under the person.
    return named;
  }
  for (const row of rows) {
    for (const attr of Object.keys(row.cells)) {
      if (geometryOfRow(row, attr)) {
        return attr;
      }
    }
  }
  return null;
}

/** Every row that has a geometry, as one feature each, keyed by the entity id. */
export function featuresOfRows(rows: RichRow[], attr: string, edits?: Record<string, Record<string, unknown>>): GeoFeature[] {
  const features: GeoFeature[] = [];
  for (const row of rows) {
    // An edited geometry is what the map shows: the panel and the cell are the same pending value
    // until it is applied (UI-67).
    const edited = edits?.[row.id]?.[attr];
    const geometry = (edited as Geometry | undefined) ?? geometryOfRow(row, attr);
    if (geometry && typeof geometry === "object" && "type" in geometry) {
      features.push({ type: "Feature", id: row.id, geometry: geometry as Geometry, properties: { edited: edited !== undefined } });
    }
  }
  return features;
}

/** The ids of the rows that carry no geometry, which the panel lists rather than hides. */
export function rowsOffTheMap(rows: RichRow[], attr: string, edits?: Record<string, Record<string, unknown>>): string[] {
  return rows.filter((row) => edits?.[row.id]?.[attr] === undefined && !geometryOfRow(row, attr)).map((row) => row.id);
}

const EARTH_RADIUS_M = 6_371_008.8;

/** Metres between two positions, over the sphere: enough for "moved 38 m" (UI-72). */
export function metresBetween(a: Position, b: Position): number {
  const toRad = Math.PI / 180;
  const φ1 = a[1] * toRad;
  const φ2 = b[1] * toRad;
  const dφ = φ2 - φ1;
  const dλ = (b[0] - a[0]) * toRad;
  const h = Math.sin(dφ / 2) ** 2 + Math.cos(φ1) * Math.cos(φ2) * Math.sin(dλ / 2) ** 2;
  return 2 * EARTH_RADIUS_M * Math.asin(Math.min(1, Math.sqrt(h)));
}

/** The vertices of a geometry, in order, whatever its nesting. */
export function verticesOf(geometry: Geometry | null): Position[] {
  if (!geometry) {
    return [];
  }
  const out: Position[] = [];
  const walk = (value: unknown): void => {
    if (!Array.isArray(value)) {
      return;
    }
    if (typeof value[0] === "number" && typeof value[1] === "number") {
      out.push(value as Position);
      return;
    }
    for (const inner of value) {
      walk(inner);
    }
  };
  walk(geometry.coordinates);
  return out;
}

/** The absolute area of a polygon's outer ring, in square metres, by the shoelace on a local plane. */
function areaOf(geometry: Geometry | null): number {
  if (!geometry || geometry.type !== "Polygon") {
    return 0;
  }
  const ring = (geometry.coordinates as Position[][])[0];
  if (!Array.isArray(ring) || ring.length < 4) {
    return 0;
  }
  const lat = ring.reduce((sum, p) => sum + p[1], 0) / ring.length;
  const mPerDegLat = 111_132.92;
  const mPerDegLon = 111_319.49 * Math.cos((lat * Math.PI) / 180);
  let sum = 0;
  for (let i = 0; i < ring.length - 1; i += 1) {
    const [x1, y1] = ring[i];
    const [x2, y2] = ring[i + 1];
    sum += x1 * mPerDegLon * (y2 * mPerDegLat) - x2 * mPerDegLon * (y1 * mPerDegLat);
  }
  return Math.abs(sum) / 2;
}

/** What changed between the endpoint's geometry and the pending one, for the review (UI-67). */
export interface GeometryDiff {
  kind: "moved" | "reshaped" | "replaced" | "same";
  /** Metres the single vertex of a point moved; only for `moved`. */
  metres?: number;
  /** Vertices gained (positive) or lost (negative); only for `reshaped`. */
  vertices?: number;
  /** Area change as a percentage, rounded; only for a polygon that was reshaped. */
  areaPercent?: number;
  from?: GeometryType;
  to?: GeometryType;
}

/**
 * The summary of one pending geometry. A point that moved says how far, a polygon that was reshaped
 * says how many vertices and how much area, and a geometry of another type altogether says so
 * rather than pretending to measure a distance between a point and an area.
 */
export function diffGeometry(before: Geometry | null, after: Geometry | null): GeometryDiff {
  if (!before || !after) {
    return { kind: "replaced", from: before?.type as GeometryType, to: after?.type as GeometryType };
  }
  if (before.type !== after.type) {
    return { kind: "replaced", from: before.type as GeometryType, to: after.type as GeometryType };
  }
  const a = verticesOf(before);
  const b = verticesOf(after);
  if (JSON.stringify(a) === JSON.stringify(b)) {
    return { kind: "same" };
  }
  if (before.type === "Point" && a.length === 1 && b.length === 1) {
    return { kind: "moved", metres: metresBetween(a[0], b[0]) };
  }
  const areaBefore = areaOf(before);
  const areaAfter = areaOf(after);
  const diff: GeometryDiff = { kind: "reshaped", vertices: b.length - a.length };
  if (areaBefore > 0) {
    diff.areaPercent = Math.round(((areaAfter - areaBefore) / areaBefore) * 100);
  }
  return diff;
}
