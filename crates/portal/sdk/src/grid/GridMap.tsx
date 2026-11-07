/**
 * The map beside the rows (UI-72): one panel, the same state as the grid.
 *
 * Not a second view of a second query. The panel draws the rows of the page that hold a geometry,
 * a click on a shape activates that row and the active row's shape is drawn as chosen, and in edit
 * mode the geometry of the active row is edited like any other cell — the change lands in the
 * grid's pending edits (UI-67) and is applied through the Endpoint by the same `apply.ts`, with the
 * person's session and the Endpoint's Policy deciding it (EP-55).
 *
 * The drawn area is the one filter the filter row cannot hold: a rectangle becomes `georel=within`
 * on the grid's query and stays visible until it is removed.
 */

import React, { useId, useMemo, useState } from "react";

import { GeoEditor } from "../geo/GeoEditor";
import type { DrawEngine, GeoLabels } from "../geo/GeoEditor";
import { GeoView } from "../geo/GeoView";
import type { GeoFeature, TileSource } from "../geo/GeoView";
import type { Geometry, GeometryType } from "../geo/validate";
import { areaQuery } from "./geoarea";
import type { GeoArea, GeoQuery } from "./geoarea";
import { diffGeometry, featuresOfRows, geometryOfRow, rowsOffTheMap } from "./mapRows";
import type { RichRow } from "./model";
// Its rules live beside the grid's: an application may show the map without the grid.
import "./grid.css";

/** Every string the panel shows, so a host translates it one key at a time. */
export interface GridMapLabels {
  panel: string;
  offTheMap: string;
  drawArea: string;
  clearArea: string;
  /** The header button while the drawing tools are open and nothing is drawn yet. */
  stopDrawing: string;
  areaAsked: string;
  areaSimplified: string;
  editGeometry: string;
  moved: string;
  reshaped: string;
  replaced: string;
  noGeometry: string;
  viewerCannotEdit: string;
  /** "Loading points: 400 / 2 310" while the map's own read pages (T-6345). */
  pointsLoading: string;
  /** "2 310 on the map" once it ended. */
  pointsShown: string;
  /** Said when the read stopped at its cap before the answer ended. */
  pointsCapped: string;
  pointsFailed: string;
  /** "Selected: Hlavná 1 (urn:…)" after a click on a point. */
  selected: string;
  /** What to do once the drawing tools are open. */
  drawHint: string;
}

export const DEFAULT_GRID_MAP_LABELS: GridMapLabels = {
  panel: "Map",
  offTheMap: "not on the map",
  drawArea: "Draw area",
  clearArea: "Clear area",
  stopDrawing: "Stop drawing",
  areaAsked: "Filtered to the drawn area",
  areaSimplified: "The filter is a simplified outline of what you drew",
  editGeometry: "Edit geometry",
  moved: "moved",
  reshaped: "reshaped",
  replaced: "replaced",
  noGeometry: "This row has no geometry yet",
  viewerCannotEdit: "You may look at this map, not change it",
  pointsLoading: "Loading points",
  pointsShown: "on the map",
  pointsCapped: "only the first ones are drawn",
  pointsFailed: "The points could not be read; the rows are still listed, try again by reopening the map",
  selected: "Selected",
  drawHint: "Pick a shape and click the map; click an area's last corner twice to finish. Escape leaves.",
};

/** What the point read says: progress while it pages, the count once it ended, or why it failed. */
export function pointsLine(state: GridMapPoints, labels: GridMapLabels): string {
  if (state.error) return `${labels.pointsFailed}: ${state.error}`;
  const count = state.points.length.toLocaleString();
  if (!state.done) return `${labels.pointsLoading}: ${count}${state.total !== undefined ? ` / ${state.total.toLocaleString()}` : ""}`;
  return `${count} ${labels.pointsShown}${state.capped ? `, ${labels.pointsCapped}` : ""}`;
}

/** The map's own read of every entity's point, as `useMapPoints` keeps it. */
export interface GridMapPoints {
  points: readonly { id: string; name?: string }[];
  total?: number;
  done: boolean;
  capped: boolean;
  error: string | null;
}

/** "Point moved 38 m", "Polygon: 2 vertices added, area +4 %", in the host's own words. */
export function summaryOf(before: Geometry | null, after: Geometry | null, labels: GridMapLabels): string {
  const diff = diffGeometry(before, after);
  switch (diff.kind) {
    case "same":
      return "";
    case "moved": {
      const metres = diff.metres ?? 0;
      const shown = metres < 10 ? metres.toFixed(1) : String(Math.round(metres));
      return `${after?.type ?? ""} ${labels.moved} ${shown} m`.trim();
    }
    case "reshaped": {
      const vertices = diff.vertices ?? 0;
      const parts: string[] = [];
      if (vertices !== 0) {
        const word = Math.abs(vertices) === 1 ? "vertex" : "vertices";
        parts.push(`${Math.abs(vertices)} ${word} ${vertices > 0 ? "added" : "removed"}`);
      }
      if (diff.areaPercent !== undefined && diff.areaPercent !== 0) {
        parts.push(`area ${diff.areaPercent > 0 ? "+" : ""}${diff.areaPercent} %`);
      }
      const detail = parts.length > 0 ? `: ${parts.join(", ")}` : "";
      return `${after?.type ?? ""} ${labels.reshaped}${detail}`.trim();
    }
    default:
      return `${labels.replaced}: ${diff.from ?? labels.noGeometry} → ${diff.to ?? labels.noGeometry}`;
  }
}

export interface GridMapProps {
  rows: RichRow[];
  /** The attribute this panel is about; the host resolved it with `mapAttrOf`. */
  attr: string;
  /** The row the grid has active, and how to move it when a shape is clicked. */
  activeId: string | null;
  onActivate: (id: string) => void;
  /** Pending cell edits, exactly as the grid holds them (UI-67). */
  edits?: Record<string, Record<string, unknown>>;
  /** A geometry a person changed; `undefined` gives the endpoint's value back. */
  onEdit?: (id: string, attr: string, value: unknown | undefined) => void;
  /** `edit` offers the editor for the active row; `view` never does. */
  mode?: "view" | "edit";
  /** The geometries this type accepts, from the DataModel's range where the space has one. */
  allowed?: readonly GeometryType[];
  /** The area the grid is filtered to, and how to set or clear it. */
  area?: GeoArea | null;
  onArea?: (area: GeoArea | null) => void;
  /** Unused since the area is drawn on the map; kept so existing hosts still compile. */
  boundsNow?: () => [number, number, number, number] | null;
  /** Open with the area drawing tools, as the location column's filter button does. */
  startDrawing?: boolean;
  labels?: Partial<GridMapLabels>;
  basemap?: string;
  /** The drawing library, injected the same way `GeoEditor` takes it. */
  engine?: DrawEngine;
  geoLabels?: Partial<GeoLabels>;
  position?: "right" | "bottom";
  /** Every entity of the type from the Endpoint's vector tiles, under the page's shapes (T-5195). */
  tiles?: TileSource | null;
  /** Every entity of the question as a point, read lean beside the page (T-6345), and how far it got. */
  points?: GeoFeature[];
  pointsState?: GridMapPoints | null;
}

const AREA_TYPES: readonly GeometryType[] = ["Polygon"];

export function GridMap({
  rows,
  attr,
  activeId,
  onActivate,
  edits,
  onEdit,
  mode = "view",
  allowed,
  area,
  onArea,
  boundsNow: _boundsNow,
  startDrawing,
  labels: labelsPartial,
  basemap,
  engine,
  geoLabels,
  position = "right",
  tiles,
  points,
  pointsState,
}: GridMapProps): React.JSX.Element {
  const labels = { ...DEFAULT_GRID_MAP_LABELS, ...labelsPartial };
  const [editing, setEditing] = useState(false);

  const features = useMemo(() => featuresOfRows(rows, attr, edits), [rows, attr, edits]);
  // The page's own shapes over the whole answer's points: a row on the page is drawn once, as itself.
  const drawn = useMemo(() => {
    if (!points || points.length === 0) return features;
    const onPage = new Set(features.map((feature) => feature.id));
    return [...points.filter((point) => !onPage.has(point.id)), ...features];
  }, [features, points]);
  const [picked, setPicked] = useState<string | null>(null);
  const pickedName = picked ? (points?.find((point) => point.id === picked)?.properties?.name as string | undefined) : undefined;
  const offTheMap = useMemo(() => rowsOffTheMap(rows, attr, edits), [rows, attr, edits]);
  const activeRow = useMemo(() => rows.find((row) => row.id === activeId) ?? null, [rows, activeId]);

  // The endpoint's geometry for the active row, and the pending one if there is one: the summary is
  // the difference between exactly those two.
  const stored = activeRow ? geometryOfRow(activeRow, attr) : null;
  const pending = activeRow ? (edits?.[activeRow.id]?.[attr] as Geometry | undefined) : undefined;
  const shown = pending ?? stored;
  const summary = pending !== undefined ? summaryOf(stored, pending ?? null, labels) : "";

  // The drawn area as the query it will become, so the panel can say when the filter stopped being
  // the shape the person drew.
  const asked: GeoQuery | null = areaQuery(area ?? null);

  const canEdit = mode === "edit" && Boolean(onEdit);
  const reasonId = `${useId()}cannot-edit`;
  const editorOpen = canEdit && editing && activeRow !== null;

  // The area filter is drawn on the map (polygon, rectangle, circle, freehand: each a Polygon that
  // becomes georel=within), not taken from the viewport (owner 2026-11-01).
  const [drawingArea, setDrawingArea] = useState(Boolean(startDrawing) && !area);
  const areaOpen = Boolean(onArea) && drawingArea && !editorOpen;

  return (
    <section className={`jc-grid-map jc-grid-map--${position}`} aria-label={labels.panel}>
      <header className="jc-grid-map__bar">
        <h3 className="jc-grid-map__title">{labels.panel}</h3>
        {onArea ? (
          <button
            type="button"
            className="jc-grid-map__action"
            aria-pressed={areaOpen}
            onClick={() => (area || drawingArea ? (onArea(null), setDrawingArea(false)) : setDrawingArea(true))}
          >
            {area ? labels.clearArea : drawingArea ? labels.stopDrawing : labels.drawArea}
          </button>
        ) : null}
        {mode === "edit" && activeRow ? (
          canEdit ? (
            <button
              type="button"
              className="jc-grid-map__action"
              aria-pressed={editorOpen}
              onClick={() => setEditing((open) => !open)}
            >
              {labels.editGeometry}
            </button>
          ) : (
            // Refused, and still in the header and in the tab order, because that is where a
            // person looks for it: hiding it and writing the reason at the foot of the panel
            // left somebody with a read-only grant looking for a control that was never drawn
            // (T-2288, UI-44). The host passes its own sentence as `viewerCannotEdit`.
            <>
              <button
                type="button"
                className="jc-grid-map__action"
                aria-disabled="true"
                aria-describedby={reasonId}
                title={labels.viewerCannotEdit}
                onClick={(event) => event.preventDefault()}
              >
                {labels.editGeometry}
              </button>
              <span id={reasonId} className="jc-grid-sr-only">
                {labels.viewerCannotEdit}
              </span>
            </>
          )
        ) : null}
      </header>

      {asked || pointsState ? (
        <div className="jc-grid-map__status">
          {asked ? (
            <p className="jc-grid-map__area" role="status">
              {labels.areaAsked}
              {asked.simplified ? ` — ${labels.areaSimplified}` : ""}
            </p>
          ) : null}
          {pointsState ? (
            <p className="jc-grid-map__points" role="status" aria-live="polite">
              {pointsLine(pointsState, labels)}
            </p>
          ) : null}
        </div>
      ) : null}

      {areaOpen ? (
        // Escape leaves the drawing tools when nothing was drawn yet; inside a shape terra-draw's
        // own Escape cancels that shape first.
        <div
          className="jc-grid-map__draw"
          onKeyDown={(event) => {
            if (event.key === "Escape" && !area) {
              event.stopPropagation();
              setDrawingArea(false);
            }
          }}
        >
        {area ? null : <p className="jc-grid-map__hint">{labels.drawHint}</p>}
        <GeoEditor
          value={area?.geometry ?? null}
          onChange={(geometry) => onArea?.(geometry && geometry.type === "Polygon" ? { geoproperty: attr, geometry } : null)}
          allowed={AREA_TYPES}
          snapTo={drawn}
          basemap={basemap}
          engine={engine}
          labels={geoLabels}
          label={labels.drawArea}
        />
        </div>
      ) : editorOpen ? (
        <>
          <GeoEditor
            value={shown}
            onChange={(geometry) => onEdit?.(activeRow.id, attr, geometry ?? undefined)}
            allowed={allowed}
            // Every other row of the page is drawn behind the edited one to draw against.
            snapTo={features.filter((feature: GeoFeature) => feature.id !== activeRow.id)}
            basemap={basemap}
            engine={engine}
            labels={geoLabels}
            label={`${labels.editGeometry}: ${activeRow.id}`}
          />
          {summary ? (
            <p className="jc-grid-map__summary" role="status">
              {summary}
            </p>
          ) : null}
        </>
      ) : (
        <GeoView
          value={drawn}
          selectedId={activeId ?? picked}
          onSelect={(id) => {
            setPicked(id);
            onActivate(id);
          }}
          basemap={basemap}
          label={labels.panel}
          tiles={tiles}
        />
      )}

      {picked && !areaOpen && !editorOpen ? (
        <p className="jc-grid-map__picked" role="status">
          {labels.selected}: {pickedName ? `${pickedName} (${picked})` : picked}
        </p>
      ) : null}

      {/* With no row active there is no control to hang the reason on, so the panel says it
          once, here; with one active the button above carries it and this would be a second
          copy of the same sentence. */}
      {mode === "edit" && !onEdit && !activeRow ? (
        <p className="jc-grid-map__note">{labels.viewerCannotEdit}</p>
      ) : null}

      {offTheMap.length > 0 ? (
        <p className="jc-grid-map__off">
          {labels.offTheMap}: {offTheMap.length}
        </p>
      ) : null}
    </section>
  );
}
