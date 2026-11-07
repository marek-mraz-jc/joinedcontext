import React, { useMemo, useState } from "react";
import { GridMap } from "./GridMap";
import { mapAttrOf } from "./mapRows";
import { featuresOfPoints, useMapPoints } from "./mapPoints";
import { TILE_THRESHOLD, useTileTemplate } from "../geo/tiles";
import type { RichRow } from "./model";
import type { EntityGrid as EntityGridApi, UseEntityGridOptions } from "./useEntityGrid";
import type { GridView } from "./useGridView";

/** Which attribute the map draws, with its tiles and lean points: the same page and question as the table. */
export function useGridMap(options: UseEntityGridOptions, grid: EntityGridApi, rows: RichRow[]) {
  // The map is a second view of the same page: it is offered only where the config asked for one
  // AND the rows actually carry a geometry, so a type without one gets no panel and no action.
  const mapConfig = options.config.map;
  // A geo column's filter cell opens the map to draw an area on, where the config shows none (owner 2026-11-01).
  const [areaMap, setAreaMap] = useState<string | null>(null);
  const mapAttr = mapConfig?.enabled ? mapAttrOf(rows, mapConfig.attr) : (areaMap ?? (grid.area ? grid.area.geoproperty : null));
  const mapPosition = mapConfig?.position ?? "right";
  // Past a count the map also draws the whole type from the Endpoint's tiles (T-5195). A tile takes
  // no query, so a narrowed answer stays on the page's shapes, and only `location` is tiled.
  const tileTemplate = useTileTemplate(
    options.source.tilesAt ?? null,
    options.config.type,
    mapAttr === "location" && !grid.narrowed && (grid.total ?? 0) > TILE_THRESHOLD,
  );
  // Every entity of the question as a point, read lean while the map is open and no tiles draw the
  // type (T-6345). The question is the grid's own without the area, so a person drawing one sees
  // what it will catch, and without the order, which a map does not need.
  const mapOpen = Boolean(mapConfig?.enabled || areaMap !== null || grid.area);
  const pointsKey = useMemo(() => {
    const asked = grid.valuesQuery("");
    return JSON.stringify({ ...asked, area: undefined, attrs: undefined });
  }, [grid.valuesQuery]);
  const pointsState = useMapPoints(options.source, pointsKey, mapAttr, mapOpen && !tileTemplate);
  const mapPoints = useMemo(() => featuresOfPoints(pointsState?.points ?? []), [pointsState]);
  return { mapConfig, areaMap, setAreaMap, mapAttr, mapPosition, tileTemplate, pointsState, mapPoints };
}

/** The map beside the rows (UI-72): shapes, area drawing, and edits where the grid edits. */
export function GridMapPanel({ view }: { view: GridView }): React.JSX.Element | null {
  const { props, options, grid, editing, selection, setDetailId, mapAttr, mapConfig, mapPosition, areaMap, tileTemplate, mapPoints, pointsState } = view;
  const { rows, state, setEdit } = grid;
  const { mapAllowed, mapBounds, mapLabels, basemap, mapEngine, geoLabels } = props;
  if (!mapAttr) return null;
  const activeRowId = rows[state.activeCell?.row ?? -1]?.id ?? null;
  return (
    <GridMap
      rows={rows}
      attr={mapAttr}
      activeId={activeRowId}
      onActivate={(id) => {
        const index = rows.findIndex((row) => row.id === id);
        if (index >= 0) {
          // The shape and the row are one selection: clicking a shape moves the grid's active
          // cell to that row, which is what the keyboard and the screen reader follow.
          grid.setActive(index, state.activeCell?.col ?? 0);
        } else if (selection.inShell) {
          // A shape from a tile whose row is on no loaded page: its detail is read by the URN.
          selection.select({ id, type: options.config.type });
        } else {
          setDetailId(id);
        }
      }}
      edits={state.edits}
      onEdit={editing ? setEdit : undefined}
      mode={editing ? "edit" : "view"}
      allowed={mapAllowed}
      area={grid.area}
      onArea={grid.setArea}
      startDrawing={areaMap !== null && !mapConfig?.enabled}
      boundsNow={mapBounds}
      labels={mapLabels}
      tiles={tileTemplate ? { template: tileTemplate, layer: options.config.type } : null}
      basemap={basemap}
      points={mapPoints}
      pointsState={pointsState}
      engine={mapEngine}
      geoLabels={geoLabels}
      position={mapPosition}
    />
  );
}
