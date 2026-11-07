/**
 * Every entity of the grid's question as a point on its map (T-6345, owner 2026-11-01).
 *
 * The rows of a page are a few dozen; the map shows the whole answer, so a person drawing an area
 * sees what it will catch. The read is lean: id, name and the one geometry, as plain values, a
 * thousand a request, until a page comes back short. Past `TILE_THRESHOLD` entities the type's
 * vector tiles draw the map instead and nothing is read here.
 */
import { useEffect, useState } from "react";

import type { GeoFeature } from "../geo/GeoView";
import type { Geometry, Position } from "../geo/validate";
import { positionsOf } from "../geo/GeoView";
import { firstCell, geometryOfRow } from "./mapRows";
import type { EntitySource, GridQuery, MapPoint, PointPage } from "./source";

/** A request's worth of points: the broker's own page limit. */
export const POINTS_PAGE = 1000;
/** The most points one map reads; past it the map says it shows the first ones. */
export const MAX_POINTS = 10_000;

/** A point where a geometry is: a Point as it is, any other shape at the mean of its positions. */
export function pointGeometry(geometry: Geometry): Geometry | null {
  if (geometry.type === "Point") return geometry;
  const positions = positionsOf(geometry.coordinates);
  if (positions.length === 0) return null;
  const lon = positions.reduce((sum, p) => sum + p[0], 0) / positions.length;
  const lat = positions.reduce((sum, p) => sum + p[1], 0) / positions.length;
  return { type: "Point", coordinates: [lon, lat] as Position } as Geometry;
}

/** One page by the source's lean read, or by its row read where it has none (a fixture, an App's own source). */
async function pageOf(source: EntitySource, q: GridQuery, attr: string, offset: number): Promise<PointPage> {
  // No page asks for a count (T-6541): the walk can outlast the gateway's 10 s cut and take the
  // first points with it; the read ends at a short page, and the grid's own count reports the total.
  if (source.points) return source.points(q, attr, { offset, limit: POINTS_PAGE, count: false });
  const page = await source.query({ ...q, attrs: ["name", attr] }, { offset, limit: POINTS_PAGE, count: false });
  const points = page.rows.flatMap((row): MapPoint[] => {
    const geometry = geometryOfRow(row, attr);
    const name = firstCell(row, "name")?.value;
    return geometry ? [{ id: row.id, name: typeof name === "string" && name !== "" ? name : undefined, geometry }] : [];
  });
  return { points, read: page.rows.length, total: page.total };
}

export interface PointsProgress {
  points: MapPoint[];
  /** The whole answer's size, where the source counted it. */
  total?: number;
  done: boolean;
  /** True when `MAX_POINTS` stopped the read before the answer ended. */
  capped: boolean;
}

/**
 * Reads every point of `q`, page after page, calling `onProgress` after each; stops at a short
 * page, at `MAX_POINTS`, or when `signal` aborts (the question changed).
 */
export async function readPoints(
  source: EntitySource,
  q: GridQuery,
  attr: string,
  onProgress: (progress: PointsProgress) => void,
  signal?: AbortSignal,
  max = MAX_POINTS,
): Promise<PointsProgress> {
  const points: MapPoint[] = [];
  let total: number | undefined;
  let offset = 0;
  for (;;) {
    const page = await pageOf(source, q, attr, offset);
    if (signal?.aborted) return { points, total, done: false, capped: false };
    if (offset === 0) total = page.total;
    points.push(...page.points);
    offset += page.read;
    const ended = page.read < POINTS_PAGE || (total !== undefined && offset >= total);
    const capped = !ended && offset >= max;
    const progress = { points: [...points], total, done: ended || capped, capped };
    onProgress(progress);
    if (progress.done || signal?.aborted) return progress;
  }
}

/** The points as map features, keyed by entity id, the name kept for the panel to say. */
export function featuresOfPoints(points: readonly MapPoint[]): GeoFeature[] {
  return points.flatMap((point) => {
    const geometry = pointGeometry(point.geometry);
    return geometry ? [{ type: "Feature" as const, id: point.id, geometry, properties: { name: point.name ?? "", point: true } }] : [];
  });
}

export interface MapPointsState extends PointsProgress {
  error: string | null;
}

/**
 * The points of the grid's question while `enabled` (the map is open and not tiled). `queryKey` is
 * the question as JSON, so a new object for the same question reads nothing again.
 */
export function useMapPoints(source: EntitySource, queryKey: string, attr: string | null, enabled: boolean): MapPointsState | null {
  const [state, setState] = useState<MapPointsState | null>(null);
  useEffect(() => {
    if (!enabled || !attr) {
      setState(null);
      return;
    }
    const abort = new AbortController();
    setState({ points: [], done: false, capped: false, error: null });
    readPoints(source, JSON.parse(queryKey) as GridQuery, attr, (progress) => {
      if (!abort.signal.aborted) setState({ ...progress, error: null });
    }, abort.signal).catch((err: unknown) => {
      if (!abort.signal.aborted) setState((last) => ({ points: last?.points ?? [], done: true, capped: false, error: err instanceof Error ? err.message : String(err) }));
    });
    return () => abort.abort();
  }, [source, queryKey, attr, enabled]);
  return state;
}
