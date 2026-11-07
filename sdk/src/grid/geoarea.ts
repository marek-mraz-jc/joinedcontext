/**
 * The drawn area as an NGSI-LD geo query (UI-72, CIM 009 §4.10).
 *
 * A person draws a rectangle or a polygon on the grid's map and the grid asks the endpoint for the
 * entities inside it. That is four query parameters — `georel`, `geometry`, `coordinates`,
 * `geoproperty` — and never a typed geometry: the filter row cannot ask for a shape, which is why
 * `opsForKind` answers `[]` for a geo column.
 *
 * `coordinates` travels in the URL, so a shape a mouse drew freehand has to be made small enough to
 * send. Two steps, in this order, because the first is lossless at the precision the surface keeps:
 * the positions are rounded to 6 decimals (~0.1 m), and only if that is still too long are vertices
 * dropped, evenly, with `simplified` in the answer so the map can say the filter is not the shape.
 */

import type { Geometry, Position } from "../geo/validate";
import { metresBetween } from "./mapRows";

/** The shape a person drew, and which attribute of the type it is asked about. */
export interface GeoArea {
  geoproperty: string;
  geometry: Geometry;
}

/**
 * How long `coordinates` may be. Browsers and proxies differ; APISIX's own default request line is
 * 4 KB and the rest of the query (type, attrs, q, paging) shares it, so the shape keeps under half.
 */
export const MAX_COORDINATES_CHARS = 1800;

/** Six decimals is ~0.1 m at this latitude: a drawn area is never more exact than that. */
const DECIMALS = 6;

function round(position: Position): Position {
  const factor = 10 ** DECIMALS;
  const rounded = position.map((n) => Math.round(n * factor) / factor);
  return rounded as Position;
}

/** The single ring of a polygon area, or `null` for a geometry that cannot be one. */
export function ringOf(geometry: Geometry): Position[] | null {
  if (geometry.type !== "Polygon") {
    return null;
  }
  const rings = geometry.coordinates as unknown;
  if (!Array.isArray(rings) || rings.length === 0 || !Array.isArray(rings[0])) {
    return null;
  }
  const ring = (rings[0] as Position[]).map(round);
  return ring.length >= 4 ? ring : null;
}

/**
 * `ring` with vertices dropped evenly until its JSON is at most `maxChars`, the first and the last
 * position kept so the ring stays closed. A ring that is short enough already comes back untouched.
 */
export function decimate(ring: Position[], maxChars: number): { ring: Position[]; simplified: boolean } {
  if (JSON.stringify([ring]).length <= maxChars) {
    return { ring, simplified: false };
  }
  // A triangle is the smallest closed ring; below that there is nothing left to ask about, and the
  // caller gets the shortest ring this loop can reach rather than an empty one.
  let step = 2;
  let kept = ring;
  while (JSON.stringify([kept]).length > maxChars && kept.length > 4) {
    const middle = ring.slice(1, -1).filter((_, index) => index % step === 0);
    kept = [ring[0], ...middle, ring[ring.length - 1]];
    step += 1;
  }
  return { ring: kept, simplified: true };
}

/** What the endpoint receives for this area: the four parameters of a geo query. */
export interface GeoQuery {
  georel: string;
  geometry: string;
  coordinates: string;
  geoproperty: string;
  /** True when vertices were dropped to fit the URL, so the map can say so (UI-72). */
  simplified: boolean;
}

/**
 * The area as query parameters, or `null` for an area that is not a polygon or has no ring — a
 * refusal rather than a query the endpoint would answer with everything.
 */
export function areaQuery(area: GeoArea | null | undefined, maxChars = MAX_COORDINATES_CHARS): GeoQuery | null {
  if (!area || !area.geoproperty.trim()) {
    return null;
  }
  const ring = ringOf(area.geometry);
  if (!ring) {
    return null;
  }
  // A drawn circle is asked as what it is, a centre and a radius (`near;maxDistance`, owner
  // 2026-11-01): exact, and a few characters where its 64-vertex outline would be simplified.
  const circle = circleOf(ring);
  if (circle) {
    return {
      georel: `near;maxDistance==${circle.metres}`,
      geometry: "Point",
      coordinates: JSON.stringify(circle.centre),
      geoproperty: area.geoproperty,
      simplified: false,
    };
  }
  const { ring: sent, simplified } = decimate(ring, maxChars);
  return {
    georel: "within",
    geometry: "Polygon",
    coordinates: JSON.stringify([sent]),
    geoproperty: area.geoproperty,
    simplified,
  };
}

/** Fewer vertices than this is a drawn polygon, not a circle's outline (terra-draw draws 64). */
const CIRCLE_MIN_VERTICES = 16;
/** How far a vertex may sit from the mean radius and still be on the circle. */
const CIRCLE_TOLERANCE = 0.02;

/**
 * The centre and radius (whole metres) of a ring that is a circle's outline: many vertices, each
 * the same distance from their mean. Any other ring, rectangles and hand-drawn polygons, is `null`.
 */
export function circleOf(ring: Position[]): { centre: Position; metres: number } | null {
  const open = ring.slice(0, -1);
  if (open.length < CIRCLE_MIN_VERTICES) return null;
  const lon = open.reduce((sum, p) => sum + p[0], 0) / open.length;
  const lat = open.reduce((sum, p) => sum + p[1], 0) / open.length;
  const centre = round([lon, lat] as Position);
  const radii = open.map((p) => metresBetween(centre, p));
  const mean = radii.reduce((sum, r) => sum + r, 0) / radii.length;
  if (mean <= 0 || radii.some((r) => Math.abs(r - mean) / mean > CIRCLE_TOLERANCE)) return null;
  return { centre, metres: Math.max(1, Math.round(mean)) };
}

/** The bounding box of a drawn rectangle as a closed ring, counter-clockwise (RFC 7946 §3.1.6). */
export function ringOfBounds(west: number, south: number, east: number, north: number): Position[] {
  return [
    [west, south],
    [east, south],
    [east, north],
    [west, north],
    [west, south],
  ];
}
