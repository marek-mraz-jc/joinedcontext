import { useCallback, useEffect, useRef } from "react";
import type { JSX } from "react";
import { HeatmapLayer, HexagonLayer } from "@deck.gl/aggregation-layers";
import { GeoJsonLayer, ScatterplotLayer } from "@deck.gl/layers";
import { MapboxOverlay } from "@deck.gl/mapbox";
import type { Layer } from "@deck.gl/core";
import type { Feature } from "geojson";
import type { IControl, Map as MapLibreMap } from "maplibre-gl";
import { MapLibreView } from "./MapLibreView";
import type { MapLayer } from "./MapLibreView";
import { plainColour, RAMP, rgbOf } from "./mapColours";

/**
 * A layer whose features have been fetched, which is what the count is read from.
 *
 * The style union is wider than the native one on purpose: `hexagon` and `heatmap` are
 * things only this overlay can draw, so they never reach the MapLibre component.
 */
export interface DenseLayer extends Omit<MapLayer, "style"> {
  style: MapLayer["style"] | "hexagon" | "heatmap";
  features: Feature[];
}

const PALETTE = RAMP.map(rgbOf);

/** The same colour the native paint falls back to when a layer encodes no value (UI-15). */
const plain = (): [number, number, number] => rgbOf(plainColour());

function scaled(layer: DenseLayer, feature: Feature): number {
  const encoding = layer.colorBy ?? layer.sizeBy;
  if (!encoding) {
    return 0;
  }
  const [min, max] = layer.colorBy?.domain ?? [0, 100];
  const value = Number(feature.properties?.[encoding.property] ?? min);
  if (!Number.isFinite(value) || max === min) {
    return 0;
  }
  return Math.min(Math.max((value - min) / (max - min), 0), 1);
}

function colorOf(layer: DenseLayer, feature: Feature): [number, number, number] {
  if (!layer.colorBy) {
    return plain();
  }
  return PALETTE[Math.round(scaled(layer, feature) * (PALETTE.length - 1))];
}

function radiusOf(layer: DenseLayer, feature: Feature): number {
  const [min, max] = layer.sizeBy?.range ?? [5, 5];
  return min + (max - min) * scaled(layer, feature);
}

/** A point's coordinates, or nothing for a feature no aggregation layer can place. */
function positionOf(feature: Feature): [number, number] | null {
  if (feature.geometry?.type !== "Point") {
    return null;
  }
  // Not destructured straight away: a Point whose `coordinates` arrived as null — an entity
  // with an empty geoproperty, which a broker answers with — threw here and took the whole
  // dashboard down with it (T-2137).
  const coordinates: unknown = feature.geometry.coordinates;
  if (!Array.isArray(coordinates)) {
    return null;
  }
  const [longitude, latitude] = coordinates;
  return Number.isFinite(longitude) && Number.isFinite(latitude) ? [longitude, latitude] : null;
}

/**
 * The deck.gl layer one Layer manifest asks for (UI-20).
 *
 * The three point layers take only the features they can place, so a mixed collection
 * aggregates what it can instead of failing on the first geometry that is not a point; the
 * lines and polygons of such a collection are drawn by `GeoJsonLayer`, which is also what a
 * dense boundary layer gets.
 */
export function deckLayerFor(layer: DenseLayer): Layer {
  const points = layer.features.filter((feature) => positionOf(feature) !== null);
  const shared = { id: layer.name, pickable: true };

  if (layer.style === "hexagon") {
    return new HexagonLayer({
      ...shared,
      data: points,
      getPosition: (feature: Feature) => positionOf(feature) ?? [0, 0],
      colorRange: PALETTE,
      extruded: true,
      radius: 100,
    });
  }
  if (layer.style === "heatmap") {
    return new HeatmapLayer({
      ...shared,
      data: points,
      getPosition: (feature: Feature) => positionOf(feature) ?? [0, 0],
      getWeight: (feature: Feature) => scaled(layer, feature),
      colorRange: PALETTE,
    });
  }
  if (layer.style === "circle") {
    return new ScatterplotLayer({
      ...shared,
      data: points,
      getPosition: (feature: Feature) => positionOf(feature) ?? [0, 0],
      getFillColor: (feature: Feature) => colorOf(layer, feature),
      getRadius: (feature: Feature) => radiusOf(layer, feature),
      radiusUnits: "pixels",
      stroked: false,
    });
  }
  return new GeoJsonLayer({
    ...shared,
    data: layer.features,
    filled: layer.style === "fill",
    getFillColor: (feature: Feature) => colorOf(layer, feature),
    getLineColor: (feature: Feature) => colorOf(layer, feature),
    getLineWidth: 2,
    lineWidthUnits: "pixels",
  });
}

export interface DeckGlOverlayProps {
  /** The layers MapLibre draws natively, below the threshold (UI-21). */
  layers: MapLayer[];
  /** The layers the overlay draws on the GPU, at or above it, features included. */
  dense: DenseLayer[];
  center?: [number, number];
  zoom?: number;
  label: string;
  /** The project whose basemap route draws the ground (AP-67). */
  project?: string;
}

/**
 * The same map, with the dense layers drawn on top of it by deck.gl (UI-20, UI-21).
 *
 * One map, not two: `MapboxOverlay` is a MapLibre control, so the basemap, the navigation
 * and the native layers stay exactly what they were and the overlay is a layer of its own
 * above them. A dashboard mixing a 200-feature boundary layer with a 60 000-point telemetry
 * layer therefore draws both, each with the renderer that suits it.
 */
export function DeckGlOverlay({
  layers,
  dense,
  center,
  zoom,
  label,
  project,
}: DeckGlOverlayProps): JSX.Element {
  // One overlay for the map's life: new dense features are handed to it, never a new map. A
  // callback that changed with `dense` rebuilt the whole map on every refetch after a pan and lost
  // the viewport the reader chose (UI-22, F-UI1-015).
  const overlay = useRef<MapboxOverlay | null>(null);
  const latest = useRef(dense);
  const attach = useCallback((map: MapLibreMap) => {
    const made = new MapboxOverlay({ layers: latest.current.map(deckLayerFor) });
    overlay.current = made;
    map.addControl(made as unknown as IControl);
    return () => {
      map.removeControl(made as unknown as IControl);
      overlay.current = null;
    };
  }, []);
  useEffect(() => {
    if (latest.current === dense) return;
    latest.current = dense;
    overlay.current?.setProps({ layers: dense.map(deckLayerFor) });
  }, [dense]);

  return (
    <MapLibreView
      layers={layers}
      center={center}
      zoom={zoom}
      label={label}
      project={project}
      onReady={attach}
    />
  );
}
