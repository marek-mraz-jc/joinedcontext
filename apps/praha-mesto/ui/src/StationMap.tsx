/**
 * Points on the served basemap, each coloured by its value on the branding ramp, with the legend
 * beside it (AP-67, T-2921). A browser without WebGL keeps the page: the map says it cannot draw
 * and the lists below carry the same rows.
 */
import { useEffect, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { mapColors, mapWorkerReady, styleFor, useClient } from "@joinedcontext/sdk";
import { legendOf } from "./praha";
import type { MapPoint } from "./praha";

export function StationMap({
  points,
  steps,
  label,
  legendTitle,
  noMap,
}: {
  points: MapPoint[];
  steps: number[];
  label: string;
  legendTitle: string;
  noMap: string;
}) {
  const { config } = useClient();
  const host = useRef<HTMLDivElement>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [ready, setReady] = useState(false);
  const [failed, setFailed] = useState(false);
  const colors = mapColors();
  const legend = legendOf(steps, colors.low, colors.high);

  useEffect(() => {
    let gone = false;
    void mapWorkerReady().then(() => {
      if (gone || !host.current || map.current) return;
      try {
        const instance = new MapLibreMap({
          container: host.current,
          style: styleFor(config.basemap),
          center: [14.43, 50.08],
          zoom: 10,
          attributionControl: { compact: true },
        });
        map.current = instance;
        instance.on("load", () => {
          instance.addSource("points", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
          instance.addLayer({
            id: "points",
            type: "circle",
            source: "points",
            paint: {
              "circle-radius": 5,
              "circle-color": ["get", "colour"],
              "circle-stroke-width": 1,
              "circle-stroke-color": colors.stroke,
            },
          });
          setReady(true);
        });
      } catch {
        setFailed(true);
      }
    });
    return () => {
      gone = true;
      map.current?.remove();
      map.current = null;
    };
    // The map is built once; the points reach it through `setData`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!ready) return;
    const features = points.map((point) => ({
      type: "Feature" as const,
      geometry: { type: "Point" as const, coordinates: point.at },
      properties: { colour: legend[legend.findLastIndex((step) => point.value >= step.from)]?.colour ?? colors.point },
    }));
    (map.current?.getSource("points") as GeoJSONSource | undefined)?.setData({ type: "FeatureCollection", features });
  }, [points, ready, legend, colors.point]);

  return (
    <figure className="station-map">
      {failed ? <p role="status">{noMap}</p> : <div ref={host} className="map" role="application" aria-label={label} />}
      <figcaption>
        <span className="legend-title">{legendTitle}</span>
        <ul className="legend">
          {legend.map((step) => (
            <li key={step.label}>
              <span className="swatch" style={{ background: step.colour }} aria-hidden="true" /> {step.label}
            </li>
          ))}
        </ul>
      </figcaption>
    </figure>
  );
}
