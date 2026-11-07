import { useEffect, useRef, useState } from "react";
import type { JSX } from "react";
import { Map as MapLibreMap, NavigationControl, Popup, setWorkerUrl } from "maplibre-gl";
import type { MapGeoJSONFeature, MapLayerMouseEvent, StyleSpecification } from "maplibre-gl";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import { useTranslation } from "react-i18next";
import "maplibre-gl/dist/maplibre-gl.css";
import { basemapColour, outlineColour, ONE_VALUE_COLOUR, plainColour, RAMP, unknownColour } from "./mapColours";

// MapLibre finds its worker beside its own module (`./maplibre-gl-worker.mjs`), which after
// bundling is /assets/, where nothing of that name is emitted: every dashboard asked for it and
// got a 404 (T-2760). Vite bundles the worker with the chunk it imports and names its address.
setWorkerUrl(workerUrl);

/** One Layer manifest, resolved against the Endpoint it reads from. */
export interface MapLayer {
  name: string;
  /** Same-origin NGSI-LD query of the Endpoint, read as GeoJSON, `geoQ` and friends included. */
  url: string;
  /**
   * The features, when the caller has already fetched them to count them (UI-21). MapLibre
   * takes a source that is either a URL it fetches or the collection itself, and handing it
   * what is already in memory is what keeps a dashboard from downloading each layer twice.
   */
  data?: unknown;
  style: "circle" | "line" | "fill";
  colorBy?: { property: string; domain?: [number, number]; palette?: string };
  sizeBy?: { property: string; range?: [number, number] };
  popupProperties?: string[];
  /**
   * Points gathered into counted circles at low zoom (T-3256): a click on one zooms to what it
   * holds. Only for a point layer; MapLibre clusters points alone.
   */
  cluster?: boolean;
  /** The popup's own button, opening the feature's entity by its `id` property (T-3256). */
  open?: { label: string; onOpen: (id: string) => void };
}

/** `[west, south, east, north]` of the viewport, as the Endpoint's `coordinates` wants it. */
export type Bbox = [number, number, number, number];

export interface MapLibreViewProps {
  layers: MapLayer[];
  /** The project whose basemap route draws the ground (AP-67); without one the ground is plain. */
  project?: string;
  center?: [number, number];
  zoom?: number;
  label: string;
  /** The viewport after every pan or zoom, for the page to ask the Endpoints again (UI-22). */
  onMoveEnd?: (bbox: Bbox) => void;
  /** Fills a box of the caller's, instead of the map's own framed height. */
  fill?: boolean;
  /**
   * Called once the map exists, for an overlay to attach itself to it (UI-20); what it
   * returns is called when the map goes away. This is how the deck.gl overlay draws on the
   * same map instead of a second one beside it.
   */
  onReady?: (map: MapLibreMap) => (() => void) | void;
}

/**
 * The ground when no basemap is reachable: one background layer and nothing fetched, since the
 * Portal's policy lets a page connect to its own origin only.
 */
export const blankStyle = (): StyleSpecification => ({
  version: 8,
  sources: {},
  layers: [
    { id: "background", type: "background", paint: { "background-color": basemapColour() } },
  ],
});


/**
 * The style a map starts from: the project's basemap route on the Portal's own origin (AP-67),
 * never a third-party style, which the Portal's `connect-src 'self'` would refuse anyway.
 */
export function mapStyleFor(project?: string): string | StyleSpecification {
  if (!project) {
    return blankStyle();
  }
  return new URL(
    `/api/v1/projects/${encodeURIComponent(project)}/basemap/default/style.json`,
    window.location.origin,
  ).href;
}

/** Banská Bystrica: the demo city, and a better first view than null island. */
const DEFAULT_CENTER: [number, number] = [19.146, 48.736];
const DEFAULT_ZOOM = 11;

/**
 * The ramp over a range. A range of one value has no ramp: MapLibre refuses an interpolate whose
 * stops do not strictly ascend and the layer draws nothing, with no word to the person (T-6442).
 */
function rampOver(property: string, [min, max]: [number, number]): unknown {
  if (min >= max) {
    return ONE_VALUE_COLOUR;
  }
  return [
    "interpolate",
    ["linear"],
    ["to-number", ["get", property], 0],
    ...RAMP.flatMap((stop, index) => [min + ((max - min) * index) / (RAMP.length - 1), stop]),
  ];
}

function paintFor(layer: MapLayer): Record<string, unknown> {
  const color = layer.colorBy
    ? [
        "case",
        // A feature without the number is unknown, not the ramp's low end (T-6442).
        ["has", layer.colorBy.property],
        rampOver(layer.colorBy.property, layer.colorBy.domain ?? [0, 100]),
        unknownColour(),
      ]
    : plainColour();

  if (layer.style === "line") {
    return { "line-color": color, "line-width": 2 };
  }
  if (layer.style === "fill") {
    return { "fill-color": color, "fill-opacity": 0.5, "fill-outline-color": outlineColour() };
  }

  const [minRadius, maxRadius] = layer.sizeBy?.range ?? [5, 5];
  const [low, high] = layer.colorBy?.domain ?? [0, 100];
  const radius =
    layer.sizeBy && low < high
      ? ["interpolate", ["linear"], ["to-number", ["get", layer.sizeBy.property], 0], low, minRadius, high, maxRadius]
      : minRadius;
  return {
    "circle-color": color,
    "circle-radius": radius,
    "circle-stroke-width": 1,
    "circle-stroke-color": outlineColour(),
  };
}

/**
 * A feature's popup, built as nodes with text only: an attribute value is whatever the data
 * holds, and none of it is ever read as markup. With `open`, a button opens the entity.
 */
export function popupNode(
  feature: Pick<MapGeoJSONFeature, "properties">,
  properties?: string[],
  open?: MapLayer["open"],
): HTMLElement {
  const props = feature.properties ?? {};
  const keys = properties?.length ? properties : Object.keys(props).filter((key) => key !== "id").slice(0, 6);
  const root = document.createElement("div");
  const list = document.createElement("dl");
  for (const key of keys) {
    const term = document.createElement("dt");
    term.textContent = key;
    const value = document.createElement("dd");
    value.textContent = props[key] === undefined || props[key] === null ? "—" : String(props[key]);
    list.append(term, value);
  }
  root.append(list);
  const id = props.id;
  if (open && typeof id === "string") {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "mt-1 text-primary-soft-fg underline";
    button.textContent = open.label;
    button.addEventListener("click", () => open.onOpen(id));
    root.append(button);
  }
  return root;
}

/** The layer a clustered source adds beside its own: the circles that grow with what they hold. */
const clusterLayers = (name: string) => [`${name}--clusters`];

/** The extent of a feature collection, or nothing when it has no coordinate. */
export function extentOf(data: unknown): Bbox | null {
  let box: Bbox | null = null;
  const visit = (value: unknown) => {
    if (!Array.isArray(value)) {
      return;
    }
    if (value.length >= 2 && typeof value[0] === "number" && typeof value[1] === "number") {
      const [x, y] = value as number[];
      if (!Number.isFinite(x) || !Number.isFinite(y)) {
        return;
      }
      box = box
        ? [Math.min(box[0], x), Math.min(box[1], y), Math.max(box[2], x), Math.max(box[3], y)]
        : [x, y, x, y];
      return;
    }
    value.forEach(visit);
  };
  const features = (data as { features?: { geometry?: { coordinates?: unknown } }[] } | undefined)?.features;
  features?.forEach((feature) => visit(feature.geometry?.coordinates));
  return box;
}

/**
 * Native MapLibre vector rendering of Layer manifests (UI-20, UI-21). Datasets of 50k
 * features and up belong to the deck.gl overlay of T-0225, not here.
 */
export function MapLibreView({
  layers,
  center,
  zoom,
  label,
  project,
  onReady,
  onMoveEnd,
  fill,
}: MapLibreViewProps): JSX.Element {
  const { t } = useTranslation();
  const container = useRef<HTMLDivElement | null>(null);
  const map = useRef<MapLibreMap | null>(null);
  const [failed, setFailed] = useState(false);
  const [loaded, setLoaded] = useState<MapLibreMap | null>(null);
  // The latest callback, so a pan does not rebuild the map.
  const moveEnd = useRef(onMoveEnd);
  useEffect(() => {
    moveEnd.current = onMoveEnd;
  }, [onMoveEnd]);
  // The layers on the map, by name, to diff the next render against.
  const drawn = useRef(new Map<string, MapLayer>());
  /** The click handlers each drawn layer added, so its removal takes them along (F-UI1-016). */
  const handlers = useRef(new Map<string, [string, "click", (event: MapLayerMouseEvent) => void][]>());
  const fitted = useRef(false);

  useEffect(() => {
    if (!container.current || map.current) {
      return;
    }
    let instance: MapLibreMap;
    try {
      instance = new MapLibreMap({
        container: container.current,
        style: mapStyleFor(project),
        center: center ?? DEFAULT_CENTER,
        zoom: zoom ?? DEFAULT_ZOOM,
        // A layer MapLibre fetches by URL reads an Endpoint's NGSI-LD API, which answers GeoJSON
        // only when asked for it (API/02 §3, T-4600).
        transformRequest: (url, kind) =>
          kind === "Source" && url.includes("/ngsi-ld/v1/entities")
            ? { url, headers: { Accept: "application/geo+json" } }
            : { url },
      });
      instance.addControl(new NavigationControl(), "top-right");
    } catch {
      // No WebGL (a locked-down browser, a headless runner) must not blank the page.
      // The state change is the whole point of the catch, so the lint rule is wrong here.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setFailed(true);
      return;
    }
    map.current = instance;
    const detach = onReady?.(instance);
    instance.on("load", () => setLoaded(instance));
    // A platform without a basemap answers the style with 404: the map falls back to the plain
    // ground once, so the layers still draw instead of waiting for a style that never loads.
    let fellBack = false;
    instance.on("error", () => {
      if (!fellBack && !instance.isStyleLoaded()) {
        fellBack = true;
        // The map's own `load` never comes once its first style failed: the plain ground's
        // `style.load` is what says the layers may be added now (T-3256).
        instance.once("style.load", () => setLoaded(instance));
        instance.setStyle(blankStyle());
      }
    });
    instance.on("moveend", () => {
      const bounds = instance.getBounds();
      moveEnd.current?.([bounds.getWest(), bounds.getSouth(), bounds.getEast(), bounds.getNorth()]);
    });

    return () => {
      detach?.();
      instance.remove();
      map.current = null;
      drawn.current.clear();
      handlers.current.clear();
      setLoaded(null);
    };
  }, [center, zoom, project, onReady]);

  // The layers are diffed against what the map holds: a refetch after a pan replaces the
  // data of a source in place, so the viewport the reader chose survives it (UI-22).
  useEffect(() => {
    const instance = loaded;
    if (!instance) {
      return;
    }
    const wanted = new Set(layers.map((layer) => layer.name));
    for (const name of [...drawn.current.keys()]) {
      if (!wanted.has(name)) {
        const was = drawn.current.get(name);
        if (was?.cluster && was.style === "circle") {
          for (const extra of clusterLayers(name)) instance.removeLayer(extra);
        }
        instance.removeLayer(name);
        instance.removeSource(name);
        drawn.current.delete(name);
        // A layer's click handlers leave with it, or one that comes back opens a popup for each
        // time it was added (F-UI1-016).
        for (const [id, type, handler] of handlers.current.get(name) ?? []) instance.off(type, id, handler);
        handlers.current.delete(name);
      }
    }
    for (const layer of layers) {
      const data = (layer.data ?? layer.url) as string;
      const before = drawn.current.get(layer.name);
      if (before) {
        if (before.data !== layer.data || before.url !== layer.url) {
          (instance.getSource(layer.name) as { setData?: (d: string) => void } | undefined)?.setData?.(data);
          drawn.current.set(layer.name, layer);
        }
        continue;
      }
      const clustered = layer.cluster === true && layer.style === "circle";
      const listen = (id: string, type: "click", handler: (event: MapLayerMouseEvent) => void) => {
        instance.on(type, id, handler);
        handlers.current.set(layer.name, [...(handlers.current.get(layer.name) ?? []), [id, type, handler]]);
      };
      instance.addSource(layer.name, {
        type: "geojson",
        data,
        ...(clustered ? { cluster: true, clusterRadius: 48, clusterMaxZoom: 14 } : {}),
      });
      instance.addLayer({
        id: layer.name,
        type: layer.style,
        source: layer.name,
        ...(clustered ? { filter: ["!", ["has", "point_count"]] } : {}),
        paint: paintFor(layer),
      } as Parameters<MapLibreMap["addLayer"]>[0]);
      if (clustered) {
        const [circles] = clusterLayers(layer.name);
        instance.addLayer({
          id: circles,
          type: "circle",
          source: layer.name,
          filter: ["has", "point_count"],
          paint: {
            "circle-color": plainColour(),
            "circle-opacity": 0.85,
            "circle-radius": ["step", ["get", "point_count"], 14, 10, 18, 100, 24, 1000, 30],
            "circle-stroke-width": 2,
            "circle-stroke-color": outlineColour(),
          },
        });
        listen(circles, "click", (event) => {
          const feature = event.features?.[0];
          const id = feature?.properties?.cluster_id;
          const source = instance.getSource(layer.name) as { getClusterExpansionZoom?: (id: number) => Promise<number> } | undefined;
          if (typeof id !== "number" || !source?.getClusterExpansionZoom || feature?.geometry.type !== "Point") {
            return;
          }
          const at = feature.geometry.coordinates as [number, number];
          void source.getClusterExpansionZoom(id).then((next) => instance.easeTo({ center: at, zoom: next }));
        });
      }
      listen(layer.name, "click", (event) => {
        const feature = event.features?.[0];
        if (!feature) {
          return;
        }
        new Popup()
          .setLngLat(event.lngLat)
          .setDOMContent(popupNode(feature, layer.popupProperties, layer.open))
          .addTo(instance);
      });
      drawn.current.set(layer.name, layer);
    }
    // The first data decides the view: a dashboard of Helsinki opens on Helsinki, not on the
    // default city, and only once, so a pan is never undone by a refetch.
    if (!fitted.current && !center) {
      const boxes = layers.map((layer) => extentOf(layer.data)).filter((box): box is Bbox => box !== null);
      if (boxes.length > 0) {
        const box = boxes.reduce((a, b) => [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[2], b[2]), Math.max(a[3], b[3])]);
        fitted.current = true;
        instance.fitBounds([box[0], box[1], box[2], box[3]], { padding: 40, maxZoom: 14, duration: 0 });
      }
    }
  }, [layers, loaded, center]);

  if (failed) {
    return (
      <div role="status" className="rounded border border-border bg-surface-subtle p-4 text-sm">
        {t("dashboards.mapUnavailable")}
      </div>
    );
  }

  return (
    <div
      ref={container}
      role="application"
      aria-label={label}
      className={`${fill ? "h-full" : "h-112 rounded border border-border"} w-full overflow-hidden`}
    />
  );
}
