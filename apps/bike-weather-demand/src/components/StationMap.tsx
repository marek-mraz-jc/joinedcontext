import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource, MapGeoJSONFeature, MapMouseEvent } from "maplibre-gl";
import { currentTokens, mapWorkerReady, NO_BASEMAP, pointOf, styleFor, useClient } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { stationBikes, stationName, stationSlots } from "../bikes";

export interface StationMapProps {
  stations: Row[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  label: string;
}

/**
 * Renders city-bike stations as circle markers on MapLibre: circles are coloured by available
 * bikes now and high-contrast borders highlight the currently selected station. Clicking selects.
 */
export function StationMap({ stations, selectedId, onSelect, label }: StationMapProps): React.JSX.Element {
  const client = useClient();
  const basemap = client.config.basemap;
  const container = useRef<HTMLDivElement | null>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [ready, setReady] = useState(false);
  const fitted = useRef(false);
  const extent = useRef<[[number, number], [number, number]] | null>(null);
  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;
  const tokens = useMemo(() => currentTokens(), []);

  const maxBikes = useMemo(() => {
    const vals = stations.map(stationBikes);
    return vals.length > 0 ? Math.max(...vals, 1) : 1;
  }, [stations]);

  const geoData = useMemo(() => {
    const features = stations
      .map((s) => {
        const pt = pointOf(s.location);
        if (!pt) return null;
        const avail = stationBikes(s);
        const slots = stationSlots(s);
        return {
          type: "Feature" as const,
          id: s.id,
          properties: {
            id: s.id,
            name: stationName(s),
            available: avail,
            slots,
            selected: s.id === selectedId ? 1 : 0,
          },
          geometry: {
            type: "Point" as const,
            coordinates: pt,
          },
        };
      })
      .filter((f): f is NonNullable<typeof f> => f !== null);

    return {
      type: "FeatureCollection" as const,
      features,
    };
  }, [stations, selectedId]);

  const allCoords = useMemo(() => {
    const coords: [number, number][] = [];
    for (const s of stations) {
      const pt = pointOf(s.location);
      if (pt) coords.push(pt);
    }
    return coords;
  }, [stations]);

  useEffect(() => {
    let gone = false;
    let instance: MapLibreMap | null = null;

    void mapWorkerReady().then(() => {
      if (gone || !container.current || map.current) return;
      instance = new MapLibreMap({
        container: container.current,
        style: styleFor(basemap),
        center: [24.94, 60.19],
        zoom: 11,
        canvasContextAttributes: { preserveDrawingBuffer: true },
      });
      map.current = instance;

      const box = new ResizeObserver(() => {
        if (gone || !instance) return;
        instance.resize();
        if (extent.current) instance.fitBounds(extent.current, { padding: 24, maxZoom: 14, duration: 0 });
      });
      box.observe(container.current);
      instance.once("remove", () => box.disconnect());
      instance.on("error", (event) => console.error("bike-weather-demand: map error", event.error?.message ?? event));

      instance.on("load", () => {
        if (gone || !instance) return;
        instance.addSource("stations", { type: "geojson", data: { type: "FeatureCollection", features: [] } });

        instance.addLayer({
          id: "stations-circle",
          type: "circle",
          source: "stations",
          paint: {
            "circle-radius": [
              "case",
              ["==", ["get", "selected"], 1],
              9,
              6,
            ],
            "circle-color": [
              "interpolate-lab",
              ["linear"],
              ["get", "available"],
              0,
              tokens.map.low,
              maxBikes,
              tokens.map.high,
            ],
            "circle-stroke-color": [
              "case",
              ["==", ["get", "selected"], 1],
              tokens.map.selected,
              tokens.map.stroke,
            ],
            "circle-stroke-width": [
              "case",
              ["==", ["get", "selected"], 1],
              3,
              1.5,
            ],
          },
        });

        instance.on("click", "stations-circle", (event: MapMouseEvent & { features?: MapGeoJSONFeature[] }) => {
          const id = event.features?.[0]?.properties?.id;
          if (typeof id === "string") {
            onSelectRef.current(id);
          }
        });

        instance.on("mouseenter", "stations-circle", () => {
          if (map.current) map.current.getCanvas().style.cursor = "pointer";
        });
        instance.on("mouseleave", "stations-circle", () => {
          if (map.current) map.current.getCanvas().style.cursor = "";
        });

        setReady(true);
      });
    });

    return () => {
      gone = true;
      instance?.remove();
      map.current = null;
    };
  }, [basemap, tokens, maxBikes]);

  useEffect(() => {
    const instance = map.current;
    if (!ready || !instance) return;

    const source = instance.getSource("stations") as GeoJSONSource | undefined;
    if (source) {
      source.setData(geoData);
    }

    instance.setPaintProperty("stations-circle", "circle-radius", [
      "case",
      ["==", ["get", "selected"], 1],
      9,
      6,
    ]);
    instance.setPaintProperty("stations-circle", "circle-stroke-color", [
      "case",
      ["==", ["get", "selected"], 1],
      tokens.map.selected,
      tokens.map.stroke,
    ]);
    instance.setPaintProperty("stations-circle", "circle-stroke-width", [
      "case",
      ["==", ["get", "selected"], 1],
      3,
      1.5,
    ]);
    instance.setPaintProperty("stations-circle", "circle-color", [
      "interpolate-lab",
      ["linear"],
      ["get", "available"],
      0,
      tokens.map.low,
      maxBikes,
      tokens.map.high,
    ]);

    if (!fitted.current && allCoords.length > 0) {
      fitted.current = true;
      const lons = allCoords.map((c) => c[0]);
      const lats = allCoords.map((c) => c[1]);
      extent.current = [
        [Math.min(...lons), Math.min(...lats)],
        [Math.max(...lons), Math.max(...lats)],
      ];
      instance.fitBounds(extent.current, { padding: 24, maxZoom: 14, duration: 0 });
    }
  }, [ready, geoData, maxBikes, tokens, allCoords]);

  return (
    <div className="jc-map">
      <div
        className="jc-map-canvas"
        ref={container}
        data-testid="jc-map"
        role="application"
        aria-label={label}
      />
      {!basemap && <span className="jc-map-notice">{NO_BASEMAP}</span>}
    </div>
  );
}
