import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap, Popup } from "maplibre-gl";
import type { GeoJSONSource, MapGeoJSONFeature, MapMouseEvent } from "maplibre-gl";
import { currentTokens, mapWorkerReady, NO_BASEMAP, styleFor, useClient } from "@joinedcontext/sdk";
import type { Hex, Place } from "../analysis";

/**
 * The hexagons and the repeat places on one map (T-3333): a hexagon's colour runs from the
 * tokens' `map.low` to `map.high` over its count, a place is a circle sized by its alerts. A click
 * names what is there in a popup built of text, never of markup the data could carry.
 */
export function HexMap({
  hexes,
  places,
  label,
  describe,
  onPlace,
  snapshot,
  height = 460,
}: {
  hexes: Hex[];
  places: Place[];
  /** What a screen reader calls the map. */
  label: string;
  /** The popup's lines for a hexagon or a place. */
  describe: (what: { kind: "hex"; hex: Hex } | { kind: "place"; place: Place }) => string[];
  /** Called with the place a click lands on, after its popup. */
  onPlace?: (place: Place) => void;
  /** Filled with a function that takes a PNG of the map as drawn, `null` when it cannot (T-3351). */
  snapshot?: { current: (() => Promise<Blob | null>) | null };
  height?: number;
}): React.JSX.Element {
  const client = useClient();
  const basemap = client.config.basemap;
  const container = useRef<HTMLDivElement | null>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [ready, setReady] = useState(false);
  const fitted = useRef(false);
  const describeRef = useRef(describe);
  describeRef.current = describe;
  const onPlaceRef = useRef(onPlace);
  onPlaceRef.current = onPlace;
  const tokens = useMemo(() => currentTokens(), []);
  const most = hexes.reduce((max, hex) => Math.max(max, hex.count), 1);

  const hexData = useMemo(
    () => ({
      type: "FeatureCollection" as const,
      features: hexes.map((hex, index) => ({
        type: "Feature" as const,
        id: index,
        properties: { index, count: hex.count },
        geometry: { type: "Polygon" as const, coordinates: [hex.ring] },
      })),
    }),
    [hexes],
  );
  const placeData = useMemo(
    () => ({
      type: "FeatureCollection" as const,
      features: places.map((place, index) => ({
        type: "Feature" as const,
        id: index,
        properties: { index, count: place.count },
        geometry: { type: "Point" as const, coordinates: [place.lon, place.lat] },
      })),
    }),
    [places],
  );
  const lookup = useRef({ hexes, places });
  lookup.current = { hexes, places };

  useEffect(() => {
    let gone = false;
    let instance: MapLibreMap | null = null;
    void mapWorkerReady().then(() => {
      if (gone || !container.current || map.current) return;
      instance = new MapLibreMap({
        container: container.current,
        style: styleFor(basemap),
        center: [24.94, 60.19],
        zoom: 9.5,
        canvasContextAttributes: { preserveDrawingBuffer: true },
      });
      map.current = instance;
      // The page lays out after the map is built: the canvas follows its box, not its first size.
      const box = container.current;
      const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(() => instance?.resize());
      observer?.observe(box);
      instance.on("remove", () => observer?.disconnect());
      instance.on("error", (event) => console.error("alerts-heatmap: map error", event.error?.message ?? event));
      instance.on("load", () => {
        if (gone || !instance) return;
        instance.addSource("hexes", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
        instance.addSource("places", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
        instance.addLayer({
          id: "hexes",
          type: "fill",
          source: "hexes",
          paint: {
            // The ramp's top is set from the data below, with the data.
            "fill-color": tokens.map.low,
            "fill-opacity": 0.65,
            "fill-outline-color": tokens.map.stroke,
          },
        });
        instance.addLayer({
          id: "places",
          type: "circle",
          source: "places",
          paint: {
            "circle-color": "rgba(0,0,0,0)",
            "circle-stroke-color": tokens.map.selected,
            "circle-stroke-width": 3,
            "circle-radius": ["interpolate", ["linear"], ["get", "count"], 3, 9, 12, 20],
          },
        });
        const popup = (event: MapMouseEvent & { features?: MapGeoJSONFeature[] }, kind: "hex" | "place") => {
          const index = event.features?.[0]?.properties?.index;
          if (typeof index !== "number" || !instance) return;
          const lines =
            kind === "hex"
              ? lookup.current.hexes[index] && describeRef.current({ kind, hex: lookup.current.hexes[index] })
              : lookup.current.places[index] && describeRef.current({ kind, place: lookup.current.places[index] });
          if (!lines) return;
          const body = document.createElement("div");
          for (const line of lines) {
            const p = document.createElement("p");
            p.textContent = line;
            body.append(p);
          }
          new Popup({ closeButton: true }).setLngLat(event.lngLat).setDOMContent(body).addTo(instance);
        };
        instance.on("click", "places", (event) => {
          popup(event, "place");
          const index = event.features?.[0]?.properties?.index;
          const place = typeof index === "number" ? lookup.current.places[index] : undefined;
          if (place) onPlaceRef.current?.(place);
        });
        instance.on("click", "hexes", (event) => {
          // A place sits on top of its hexagon: its own popup answers that click.
          if (instance?.queryRenderedFeatures(event.point, { layers: ["places"] }).length) return;
          popup(event, "hex");
        });
        if (snapshot) {
          // The drawing buffer is kept (preserveDrawingBuffer), so the canvas holds the last frame.
          snapshot.current = () =>
            new Promise((resolve) => {
              try {
                instance?.getCanvas().toBlob((png) => resolve(png), "image/png");
              } catch {
                resolve(null);
              }
            });
        }
        setReady(true);
      });
    });
    return () => {
      gone = true;
      if (snapshot) snapshot.current = null;
      instance?.remove();
      map.current = null;
    };
    // The map is built once; its data and colours reach it through the effect below.
  }, [basemap, tokens, snapshot]);

  useEffect(() => {
    const instance = map.current;
    if (!ready || !instance) return;
    (instance.getSource("hexes") as GeoJSONSource | undefined)?.setData(hexData);
    (instance.getSource("places") as GeoJSONSource | undefined)?.setData(placeData);
    instance.setPaintProperty("hexes", "fill-color", ["interpolate", ["linear"], ["get", "count"], 1, tokens.map.low, Math.max(2, most), tokens.map.high]);
    if (!fitted.current && hexes.length > 0) {
      fitted.current = true;
      const lons = hexes.flatMap((hex) => hex.ring.map((p) => p[0]));
      const lats = hexes.flatMap((hex) => hex.ring.map((p) => p[1]));
      instance.fitBounds(
        [
          [Math.min(...lons), Math.min(...lats)],
          [Math.max(...lons), Math.max(...lats)],
        ],
        { padding: 24, maxZoom: 13, duration: 0 },
      );
    }
  }, [ready, hexData, placeData, hexes, most, tokens]);

  return (
    <div className="jc-map" style={{ height }}>
      <div className="jc-map-canvas" ref={container} data-testid="jc-map" role="application" aria-label={label} />
      {!basemap && <span className="jc-map-notice">{NO_BASEMAP}</span>}
    </div>
  );
}
