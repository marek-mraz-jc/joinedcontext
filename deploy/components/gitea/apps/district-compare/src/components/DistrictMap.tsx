import { useEffect, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import type { GeoJSONSource, MapGeoJSONFeature, MapMouseEvent } from "maplibre-gl";
import { currentTokens, mapWorkerReady, NO_BASEMAP, styleFor, useClient } from "@joinedcontext/sdk";
import type { DistrictOutput, Measure } from "../districts";

// The source's own GeoJSON types, by way of its `setData`: naming the `GeoJSON` namespace needs
// @types/geojson, which the build lane's store does not carry for this App (T-3335, dev 00137db).
type Collection = Extract<Parameters<GeoJSONSource["setData"]>[0], { type: "FeatureCollection" }>;
type Geometry = Collection["features"][number]["geometry"];
import { choroplethValue } from "../districts";

export interface DistrictMapProps {
  districts: DistrictOutput[];
  geometries: Map<string, unknown>;
  selectedCodes: string[];
  measure: Measure;
  onToggle: (code: string) => void;
  label: string;
}

function extractCoordinates(geometry: unknown): [number, number][] {
  const out: [number, number][] = [];
  function walk(coords: unknown) {
    if (!Array.isArray(coords)) return;
    if (coords.length >= 2 && typeof coords[0] === "number" && typeof coords[1] === "number") {
      out.push([coords[0], coords[1]]);
      return;
    }
    for (const item of coords) {
      walk(item);
    }
  }
  if (typeof geometry === "object" && geometry !== null && "coordinates" in geometry) {
    walk((geometry as { coordinates: unknown }).coordinates);
  }
  return out;
}

/**
 * Renders district polygons in MapLibre: choropleth fill by the chosen measure and
 * high-contrast borders for selected districts. Clicking any district toggles its selection.
 */
export function DistrictMap({
  districts,
  geometries,
  selectedCodes,
  measure,
  onToggle,
  label,
}: DistrictMapProps): React.JSX.Element {
  const client = useClient();
  const basemap = client.config.basemap;
  const container = useRef<HTMLDivElement | null>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [ready, setReady] = useState(false);
  const fitted = useRef(false);
  // The districts' extent, kept so a resize can fit them again.
  const extent = useRef<[[number, number], [number, number]] | null>(null);
  const onToggleRef = useRef(onToggle);
  onToggleRef.current = onToggle;
  const tokens = useMemo(() => currentTokens(), []);

  const values = useMemo(
    () => districts.map((d) => choroplethValue(d, measure)).filter((v): v is number => v !== null),
    [districts, measure],
  );
  const minVal = values.length > 0 ? Math.min(...values) : 0;
  const maxVal = values.length > 0 ? Math.max(...values, minVal + 1) : 1;

  const geoData = useMemo(() => {
    const features = districts
      .map((d) => {
        const geo = geometries.get(d.code);
        if (!geo) return null;
        const val = choroplethValue(d, measure);
        return {
          type: "Feature" as const,
          id: d.code,
          properties: {
            code: d.code,
            name: d.name,
            value: val ?? 0,
            hasValue: val !== null,
            selected: selectedCodes.includes(d.code) ? 1 : 0,
          },
          geometry: geo as Geometry,
        };
      })
      .filter((f): f is NonNullable<typeof f> => f !== null);

    return {
      type: "FeatureCollection" as const,
      features,
    };
  }, [districts, geometries, measure, selectedCodes]);

  const allCoords = useMemo(() => {
    const coords: [number, number][] = [];
    for (const d of districts) {
      const geo = geometries.get(d.code);
      if (geo) coords.push(...extractCoordinates(geo));
    }
    return coords;
  }, [districts, geometries]);

  useEffect(() => {
    let gone = false;
    let instance: MapLibreMap | null = null;

    void mapWorkerReady().then(() => {
      if (gone || !container.current || map.current) return;
      instance = new MapLibreMap({
        container: container.current,
        style: styleFor(basemap),
        center: [24.94, 60.19],
        zoom: 10,
        canvasContextAttributes: { preserveDrawingBuffer: true },
      });
      map.current = instance;
      // MapLibre follows the window, not its box: the box grows once the page around it is laid
      // out (the table, the chart), and a phone's map was drawn in half of it, cut off at the side.
      const box = new ResizeObserver(() => {
        if (gone || !instance) return;
        instance.resize();
        if (extent.current) instance.fitBounds(extent.current, { padding: 24, maxZoom: 13, duration: 0 });
      });
      box.observe(container.current);
      instance.once("remove", () => box.disconnect());
      instance.on("error", (event) => console.error("district-compare: map error", event.error?.message ?? event));
      instance.on("load", () => {
        if (gone || !instance) return;
        instance.addSource("districts", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
        instance.addLayer({
          id: "districts-fill",
          type: "fill",
          source: "districts",
          paint: {
            "fill-color": tokens.map.low,
            "fill-opacity": 0.65,
          },
        });
        instance.addLayer({
          id: "districts-line",
          type: "line",
          source: "districts",
          paint: {
            "line-color": tokens.map.stroke,
            "line-width": 1,
          },
        });

        instance.on("click", "districts-fill", (event: MapMouseEvent & { features?: MapGeoJSONFeature[] }) => {
          const code = event.features?.[0]?.properties?.code;
          if (typeof code === "string") {
            onToggleRef.current(code);
          }
        });

        instance.on("mouseenter", "districts-fill", () => {
          if (map.current) map.current.getCanvas().style.cursor = "pointer";
        });
        instance.on("mouseleave", "districts-fill", () => {
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
  }, [basemap, tokens]);

  useEffect(() => {
    const instance = map.current;
    if (!ready || !instance) return;

    const source = instance.getSource("districts") as GeoJSONSource | undefined;
    if (source) {
      source.setData(geoData);
    }

    if (values.length > 0) {
      instance.setPaintProperty("districts-fill", "fill-color", [
        "case",
        ["get", "hasValue"],
        [
          // Blended in Lab, so the middle of the scale is not the grey of a plain RGB mix.
          "interpolate-lab",
          ["linear"],
          ["get", "value"],
          minVal,
          tokens.map.low,
          maxVal,
          tokens.map.high,
        ],
        tokens.color.card,
      ]);
    } else {
      instance.setPaintProperty("districts-fill", "fill-color", tokens.color.card);
    }

    instance.setPaintProperty("districts-line", "line-color", [
      "case",
      ["==", ["get", "selected"], 1],
      tokens.map.selected,
      tokens.map.stroke,
    ]);
    instance.setPaintProperty("districts-line", "line-width", [
      "case",
      ["==", ["get", "selected"], 1],
      3,
      1,
    ]);

    if (!fitted.current && allCoords.length > 0) {
      fitted.current = true;
      const lons = allCoords.map((c) => c[0]);
      const lats = allCoords.map((c) => c[1]);
      extent.current = [
        [Math.min(...lons), Math.min(...lats)],
        [Math.max(...lons), Math.max(...lats)],
      ];
      instance.fitBounds(extent.current, { padding: 24, maxZoom: 13, duration: 0 });
    }
  }, [ready, geoData, values.length, minVal, maxVal, tokens, allCoords]);

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
