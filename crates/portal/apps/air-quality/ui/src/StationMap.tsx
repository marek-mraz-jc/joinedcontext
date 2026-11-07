/**
 * The stations on a map, each coloured by its air quality index band, with the bands in words
 * beside it (T-2925, UI-30). Clicking a station selects it, as its button in the legend does.
 */
import { useEffect, useMemo, useRef } from "react";
import type { JSX } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource, MapLayerMouseEvent } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { mapColors, NO_LOCATIONS, styleFor } from "@joinedcontext/sdk";
import type { Station } from "./api";
import { BAND_COLOUR, BAND_LABEL, BANDS, bandOf, stationFeatures } from "./quality";

const SOURCE = "stations";
/** Helsinki, where the `helsinki` space's stations are, when no station has a position. */
const HELSINKI: [number, number] = [24.94, 60.17];

/**
 * The project's basemap style, from the `#jc-config` the Portal writes into an App's page (AP-67,
 * T-2928); `undefined` when the page has none or it does not parse, and the map keeps the page's
 * plain background.
 */
export function basemapOf(doc: Document = document): string | undefined {
  try {
    const config = JSON.parse(doc.getElementById("jc-config")?.textContent || "{}") as { basemap?: unknown };
    return typeof config.basemap === "string" && config.basemap.trim() !== "" ? config.basemap : undefined;
  } catch {
    return undefined;
  }
}

export function StationMap({
  stations,
  selected,
  onSelect,
  basemap = basemapOf(),
}: {
  stations: Station[];
  selected: string | null;
  onSelect: (id: string) => void;
  /** A style URL; the page's `#jc-config` by default, else the plain background. */
  basemap?: string;
}): JSX.Element {
  const holder = useRef<HTMLDivElement>(null);
  const map = useRef<MapLibreMap | null>(null);
  const select = useRef(onSelect);
  select.current = onSelect;
  const features = useMemo(() => stationFeatures(stations), [stations]);
  // The map's load handler reads the stations of the moment it fires, not of its first render.
  const latest = useRef(features);
  latest.current = features;

  useEffect(() => {
    if (!holder.current) return;
    const first = latest.current.features[0]?.geometry.coordinates as [number, number] | undefined;
    const drawn = new MapLibreMap({ container: holder.current, style: styleFor(basemap), center: first ?? HELSINKI, zoom: 10 });
    map.current = drawn;
    drawn.on("load", () => {
      drawn.addSource(SOURCE, { type: "geojson", data: stationFeatures([]) });
      drawn.addLayer({
        id: SOURCE,
        type: "circle",
        source: SOURCE,
        paint: {
          "circle-radius": 8,
          "circle-color": ["get", "colour"],
          "circle-stroke-color": mapColors().stroke,
          "circle-stroke-width": 2,
        },
      });
      (drawn.getSource(SOURCE) as GeoJSONSource).setData(latest.current);
    });
    drawn.on("click", SOURCE, (event: MapLayerMouseEvent) => {
      const id = event.features?.[0]?.properties?.id;
      if (typeof id === "string") select.current(id);
    });
    return () => {
      drawn.remove();
      map.current = null;
    };
    // The map is built once per basemap; the stations reach it through setData below.
  }, [basemap]);

  useEffect(() => {
    const source = map.current?.getSource(SOURCE) as GeoJSONSource | undefined;
    source?.setData(features);
  }, [features]);

  return (
    <div className="station-map">
      <div ref={holder} className="map" role="region" aria-label="Stations on a map, coloured by air quality index" />
      {features.features.length === 0 && <p role="status">{NO_LOCATIONS}</p>}
      <ul className="legend" aria-label="Air quality index">
        {BANDS.map((band) => (
          <li key={band}>
            <span className="swatch" style={{ background: BAND_COLOUR[band] }} aria-hidden="true" />
            {BAND_LABEL[band]}
          </li>
        ))}
      </ul>
      <ul className="stations" aria-label="Stations">
        {stations.map((station) => {
          const band = bandOf(station.airQualityIndex);
          return (
            <li key={station.id}>
              <button type="button" aria-pressed={selected === station.id} onClick={() => onSelect(station.id)}>
                <span className="swatch" style={{ background: BAND_COLOUR[band] }} aria-hidden="true" />
                {station.name ?? station.id}: {BAND_LABEL[band]}
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
