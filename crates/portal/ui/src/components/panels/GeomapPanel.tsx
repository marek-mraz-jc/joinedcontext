/**
 * A Grafana `geomap` panel with a markers layer, the one layer daiya admits (validation.rs
 * `geomap.layerType`), drawn by the Portal's MapLibre map over daiya's table frame: a circle per
 * row with a location, in the layer's fixed colour, size and opacity, the tooltip from the row's
 * name and values, and the view fitted to the markers. What the map cannot honour (another
 * basemap, a fixed view, a location mode it does not read) is said in one line above it.
 */
import { useCallback, useMemo, useState } from "react";
import type { JSX, ReactNode } from "react";
import { ScaleControl } from "maplibre-gl";
import type { Map as MapLibreMap } from "maplibre-gl";
import { useTranslation } from "react-i18next";
import { MapLibreView } from "../dashboards/MapLibreView";
import type { MapLayer } from "../dashboards/MapLibreView";
import { Icon } from "../ui";
import { useIsPhone } from "../../phone";
import type { Frame, FrameField } from "./frames";
import { displayNameOf } from "./frames";
import type { FieldConfigSource } from "./fieldConfig";
import { colorOf, displayValue, settingsFor } from "./fieldConfig";

/** Grafana's markers defaults where the layer names none (`green`, 5 px, 0.4). */
const DEFAULT_MARKER = { color: "green", size: 5, opacity: 0.4 };
/** The tallest map on a phone, in px: a panel's desktop height is mostly empty ground there. */
const PHONE_HEIGHT = 520;
/** The fields a tooltip shows besides the name, Grafana's way: the row's values, a few. */
const TOOLTIP_FIELDS = 6;

type Point = [number, number];

/** An ISO 8601 instant written as text: a date, a time and a zone. */
const ISO_INSTANT = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:?\d{2})$/;

interface LayerOptions {
  type?: string;
  name?: string;
  tooltip?: boolean;
  location?: { mode?: string; latitude?: string; longitude?: string; geojson?: string };
  config?: { style?: { color?: { fixed?: string }; size?: { fixed?: number }; opacity?: number } };
}

/** What the panel draws, and the words for what it could not. */
export interface Markers {
  features: { type: "FeatureCollection"; features: { type: "Feature"; geometry: { type: "Point"; coordinates: Point }; properties: Record<string, string> }[] };
  rows: number;
  marker: { color: string; size: number; opacity: number };
  popupProperties: string[];
  popup: boolean;
  /** The tooltip's heading: the row's name, when the frame has one. */
  titleProperty?: string;
  /** The heading's time after the name: readings of one vehicle differ by it. */
  timeProperty?: string;
  /** Keys of `dashboardView.geomap.*` with their values, one note each. */
  notes: { key: string; values?: Record<string, string | number> }[];
}

/** A GeoJSON Point's `[lon, lat]`, or nothing. */
function pointOf(value: unknown): Point | null {
  const geometry = typeof value === "string" ? safeParse(value) : value;
  const coordinates = (geometry as { type?: unknown; coordinates?: unknown } | null)?.type === "Point" ? (geometry as { coordinates: unknown }).coordinates : null;
  if (!Array.isArray(coordinates)) return null;
  const [lon, lat] = coordinates as unknown[];
  return typeof lon === "number" && typeof lat === "number" && Number.isFinite(lon) && Number.isFinite(lat) ? [lon, lat] : null;
}

function safeParse(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

const byName = (fields: FrameField[], names: (string | undefined)[]) => fields.find((field) => names.some((name) => name && field.name.toLowerCase() === name.toLowerCase()));

/** The location of each row of one frame by the layer's `location.mode`, or null when the mode is not read. */
function locator(frame: Frame, location: LayerOptions["location"]): { at: (row: number) => Point | null; used: FrameField[] } | null {
  const mode = location?.mode ?? "auto";
  const numbers = (lat?: FrameField, lon?: FrameField) =>
    lat && lon
      ? {
          at: (row: number) => {
            const [y, x] = [Number(lat.values[row]), Number(lon.values[row])];
            return lat.values[row] != null && lon.values[row] != null && Number.isFinite(x) && Number.isFinite(y) ? ([x, y] as Point) : null;
          },
          used: [lat, lon],
        }
      : null;
  if (mode === "coords") return numbers(byName(frame.fields, [location?.latitude]), byName(frame.fields, [location?.longitude]));
  if (mode === "geojson" || mode === "auto") {
    const named = byName(frame.fields, [location?.geojson, "location", "geometry"]);
    const geo = named ?? frame.fields.find((field) => field.type === "geo" || field.values.some((value) => pointOf(value) !== null));
    if (geo) return { at: (row) => pointOf(geo.values[row]), used: [geo] };
    if (mode === "auto") return numbers(byName(frame.fields, ["latitude", "lat"]), byName(frame.fields, ["longitude", "lon", "lng"]));
    return null;
  }
  return null;
}

/** The markers of a geomap panel's first layer over its frames. */
export function markersOf(frames: Frame[], options: Record<string, unknown> | undefined, fieldConfig?: FieldConfigSource, locale = "en"): Markers {
  const layer = ((options?.layers as LayerOptions[] | undefined) ?? [])[0] ?? {};
  const style = layer.config?.style;
  const marker = {
    color: colorOf(style?.color?.fixed ?? DEFAULT_MARKER.color),
    size: typeof style?.size?.fixed === "number" ? style.size.fixed : DEFAULT_MARKER.size,
    opacity: typeof style?.opacity === "number" ? style.opacity : DEFAULT_MARKER.opacity,
  };
  const notes: Markers["notes"] = [];
  const basemap = (options?.basemap as { type?: string } | undefined)?.type;
  if (basemap && basemap !== "default") notes.push({ key: "basemap", values: { type: basemap } });
  const view = (options?.view as { id?: string } | undefined)?.id;
  if (view && view !== "fit") notes.push({ key: "view" });
  if (((options?.layers as unknown[] | undefined)?.length ?? 0) > 1) notes.push({ key: "layers", values: { n: (options?.layers as unknown[]).length } });

  const features: Markers["features"]["features"] = [];
  const shown = new Set<string>();
  let titleProperty: string | undefined;
  let timeProperty: string | undefined;
  let rows = 0;
  let unread = false;
  for (const frame of frames) {
    const length = Math.max(0, ...frame.fields.map((field) => field.values.length));
    rows += length;
    const find = locator(frame, layer.location);
    if (!find) {
      unread = true;
      continue;
    }
    // The tooltip: the name first, then the row's own values; never the location itself.
    const listed = frame.fields.filter((field) => !find.used.includes(field) && field.name !== "id" && field.name !== "type" && field.type !== "json" && field.type !== "geo");
    const name = byName(listed, ["name"]);
    const ordered = name ? [name, ...listed.filter((field) => field !== name)] : listed;
    if (name) titleProperty ??= displayNameOf(name, frame);
    const tooltip = ordered.slice(0, TOOLTIP_FIELDS + (name ? 1 : 0));
    for (let row = 0; row < length; row++) {
      const at = find.at(row);
      if (!at) continue;
      const properties: Record<string, string> = {};
      const id = frame.fields.find((field) => field.name === "id")?.values[row];
      if (typeof id === "string") properties.id = id;
      // Readings of one vehicle stack under one name: the heading carries the row's time beside it,
      // and a row repeating that time (observedAt, a `dateObserved` kept as text) is left out.
      const time = name && frame.fields.find((field) => field.type === "time");
      const when = name && time && time.values[row] != null ? displayValue(settingsFor(fieldConfig, time, frame.refId), time.values[row], locale).text : undefined;
      for (const field of tooltip) {
        if (when && field === time) continue;
        const label = displayNameOf(field, frame);
        const value = field.values[row];
        const settings = settingsFor(fieldConfig, field, frame.refId);
        // A date the source keeps as text (`dateObserved`) reads like the time fields beside it.
        const instant = typeof value === "string" && ISO_INSTANT.test(value) ? Date.parse(value) : NaN;
        const text = Number.isNaN(instant) || settings.unit ? displayValue(settings, value, locale).text : displayValue({ ...settings, unit: "dateTimeAsIso" }, instant, locale).text;
        if (when && field !== name && text === when) continue;
        shown.add(label);
        properties[label] = text;
      }
      if (when && time) {
        timeProperty ??= displayNameOf(time, frame);
        properties[displayNameOf(time, frame)] = when;
      }
      features.push({ type: "Feature", geometry: { type: "Point", coordinates: at }, properties });
    }
  }
  if (unread) notes.push({ key: "mode", values: { mode: layer.location?.mode ?? "auto" } });
  else if (features.length > 0 && features.length < rows) notes.push({ key: "missing", values: { n: rows - features.length, rows } });
  return { features: { type: "FeatureCollection", features }, rows, marker, popupProperties: [...shown], popup: layer.tooltip !== false, titleProperty, timeProperty, notes };
}

/**
 * Where a map would be, outlined, around a map panel's state: the panel reads as an empty map and
 * keeps the map's height, so a refresh that empties it does not move the grid. A phone's panel
 * takes its content's height, as every other panel's state does there.
 */
export function MapFrame({ height, children }: { height: number; children: ReactNode }): JSX.Element {
  const phone = useIsPhone();
  return (
    // The state's badge and title a size up: on a desktop they stand in a whole map's room.
    <div
      className="h-full rounded-sm border border-dashed border-border bg-surface-subtle [&_[data-empty-state]>p+p]:text-body [&_[data-empty-state]>p+p]:text-fg sm:[&_[data-empty-state]>p:first-of-type]:text-title [&_[data-empty-state]>span]:size-12 [&_[data-empty-state]>span>svg]:size-6 sm:[&_[data-empty-state]>span]:size-14 sm:[&_[data-empty-state]>span>svg]:size-7"
      style={phone ? undefined : { height }}
    >
      {children}
    </div>
  );
}

export interface GeomapPanelProps {
  title: string;
  frames: Frame[];
  fieldConfig?: FieldConfigSource;
  options?: Record<string, unknown>;
  height: number;
  locale: string;
  /** The project whose basemap route draws the ground; the plain ground without it. */
  project?: string;
  /** The panel's state when no row has a location: its title and what to do (PanelView's PanelState). */
  empty: (title: string, description: string) => JSX.Element;
}

export function GeomapPanel({ title, frames, fieldConfig, options, height, locale, project, empty }: GeomapPanelProps): JSX.Element {
  const { t } = useTranslation();
  const phone = useIsPhone();
  // On the plain ground the markers float on nothing: the caption says why and the scale gives distance.
  const [plain, setPlain] = useState(false);
  const onPlainGround = useCallback(() => setPlain(true), []);
  const onReady = useCallback((map: MapLibreMap) => {
    const scale = new ScaleControl({ unit: "metric" });
    map.addControl(scale, "bottom-left");
    return () => {
      map.removeControl(scale);
    };
  }, []);
  const markers = useMemo(() => markersOf(frames, options, fieldConfig, locale), [frames, options, fieldConfig, locale]);
  const layers = useMemo<MapLayer[]>(
    () => [{ name: "markers", url: "", data: markers.features, style: "circle", marker: markers.marker, popupProperties: markers.popupProperties, popup: markers.popup, titleProperty: markers.titleProperty, timeProperty: markers.timeProperty }],
    [markers],
  );
  if (markers.features.features.length === 0) {
    const mode = markers.notes.find((note) => note.key === "mode");
    return empty(t("dashboardView.geomap.noneTitle"), mode ? t("dashboardView.geomap.mode", mode.values) : t("dashboardView.geomap.none", { rows: markers.rows }));
  }
  return (
    <div className="flex h-full min-h-0 flex-col gap-1" style={{ height: phone ? Math.min(height, PHONE_HEIGHT) : height }}>
      {/* What the map shows and what it could not, above it: an overlay would sit under the map's controls on a phone. */}
      <p className="text-body text-fg" data-geomap-caption="">
        {t("dashboardView.geomap.count", { n: markers.features.features.length })}
        {plain ? ` · ${t("dashboardView.geomap.plain")}` : null}
      </p>
      {markers.notes.length > 0 ? (
        <p className="flex min-w-0 items-start gap-1 text-body text-fg" data-geomap-note="">
          <Icon name="info" className="mt-0.5 size-4 shrink-0" />
          <span className="min-w-0">{markers.notes.map((note) => t(`dashboardView.geomap.${note.key}`, note.values)).join(" ")}</span>
        </p>
      ) : null}
      <div className="min-h-0 flex-1 overflow-hidden rounded-sm">
        <MapLibreView layers={layers} label={t("dashboardView.geomap.label", { title, n: markers.features.features.length })} project={project} fill onReady={onReady} onPlainGround={onPlainGround} />
      </div>
    </div>
  );
}
