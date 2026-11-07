/**
 * Draw, reshape and correct a geometry (UI-72, SDK-29, T-1442).
 *
 * A GeoProperty is the one attribute a person cannot type: the explorer shows it as a line of JSON,
 * and a boundary drawn by hand into a textarea is a bow tie waiting to be saved. This is the map
 * with a toolbar — point, line, polygon, rectangle, circle and freehand where the host allows them,
 * select to move a shape or drag, insert and delete a vertex — **and the same geometry as a table
 * of numbers beside it**, because a map is a pointing device and this attribute has to be editable
 * by keyboard too (UI-05). Every action of the map is possible from the table.
 *
 * Nothing leaves the component unchecked: `checkGeometry` runs before every `onChange`, so a ring
 * is closed, the winding is normalised, and a bow tie or a hole outside its shell is a message on
 * screen instead of a write. A paste or an upload goes through `geometryOf` first, which is the
 * same check plus the Feature and FeatureCollection shapes somebody exported from elsewhere.
 */
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Map as MapLibreMap } from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { mapWorkerReady, NO_BASEMAP, styleFor } from "../sdk/map";
import { mapColors } from "../sdk/tokens";
import { GEOMETRY_TYPES, checkGeometry, geometryOf } from "./validate";
import type { GeoFinding, Geometry, GeometryType, Position } from "./validate";
import type { GeoFeature } from "./GeoView";
import { boundsOf, positionsOf } from "./GeoView";

/** A drawing mode of the toolbar: what it is called here, and the geometry it produces. */
export interface DrawMode {
  /** The mode's name, which is also Terra Draw's own name for it. */
  name: string;
  /** What the mode draws, so a mode is offered only when that type is allowed. */
  produces: GeometryType;
  /** The button's label. */
  label: string;
}

/**
 * Every mode this editor can offer. A host narrows them with `allowed`: a field that holds a
 * district boundary offers the three polygon modes and not the point.
 */
export const DRAW_MODES: DrawMode[] = [
  { name: "point", produces: "Point", label: "Point" },
  { name: "linestring", produces: "LineString", label: "Line" },
  { name: "polygon", produces: "Polygon", label: "Area" },
  { name: "rectangle", produces: "Polygon", label: "Rectangle" },
  { name: "circle", produces: "Polygon", label: "Circle" },
  { name: "freehand", produces: "Polygon", label: "Freehand" },
];

/** Every string this editor puts on screen, so a host translates it without forking the file. */
export interface GeoLabels {
  tools: string;
  select: string;
  undo: string;
  redo: string;
  remove: string;
  removeShape: string;
  point: string;
  longitude: string;
  latitude: string;
  noGeometry: string;
  paste: string;
  take: string;
  upload: string;
  notJson: string;
  tooLarge: string;
  modes: Record<string, string>;
}

/** English, which is what a host that passes no labels gets. */
export const DEFAULT_GEO_LABELS: GeoLabels = {
  tools: "Drawing tools",
  select: "Select",
  undo: "Undo",
  redo: "Redo",
  remove: "Remove",
  removeShape: "Delete the shape",
  point: "Point",
  longitude: "Longitude",
  latitude: "Latitude",
  noGeometry: "No geometry",
  paste: "Paste a geometry, a Feature or a FeatureCollection",
  take: "Take it",
  upload: "Upload a GeoJSON file",
  notJson: "that is not JSON",
  tooLarge: "that file is larger than 1 MB",
  modes: { point: "Point", linestring: "Line", polygon: "Area", rectangle: "Rectangle", circle: "Circle", freehand: "Freehand" },
};

/**
 * The most GeoJSON this editor will parse (SDK-29 security line). A geometry of `MAX_VERTICES`
 * positions is about 300 kB of text; a megabyte is a whole country's boundaries pasted in, and
 * `JSON.parse` of an arbitrarily large string is the one denial of service a person can cause here
 * by accident. It is parsed as data and never evaluated, and no URL of any kind is read out of it.
 */
export const MAX_GEOJSON_BYTES = 1_000_000;

/** The map source of the read-only features drawn against (`snapTo`). */
const REFERENCE = "geo-editor-reference";

function showReference(map: MapLibreMap, features: GeoFeature[] | undefined): void {
  const source = map.getSource?.(REFERENCE) as { setData?: (data: unknown) => void } | undefined;
  source?.setData?.({ type: "FeatureCollection", features: (features ?? []).map((feature) => ({ ...feature, properties: { id: feature.id } })) });
}

/** The modes a host may offer for a type, which is what the toolbar is built from. */
export function modesFor(allowed: readonly GeometryType[]): DrawMode[] {
  return DRAW_MODES.filter((mode) => allowed.includes(mode.produces));
}

/** One row of the coordinate table: where the position sits in `coordinates`, and its numbers. */
export interface CoordinateRow {
  /** The indices from `coordinates` down to the position, so it can be written back. */
  at: number[];
  position: Position;
}

/** The geometry as a flat list of positions a table can show and a keyboard can edit. */
export function rowsOf(geometry: Geometry | null): CoordinateRow[] {
  if (!geometry) {
    return [];
  }
  const rows: CoordinateRow[] = [];
  const walk = (value: unknown, at: number[]): void => {
    if (!Array.isArray(value)) {
      return;
    }
    if (typeof value[0] === "number") {
      rows.push({ at, position: value as Position });
      return;
    }
    value.forEach((part, index) => walk(part, [...at, index]));
  };
  walk(geometry.coordinates, []);
  return rows;
}

/** The geometry with one position replaced, leaving every other number as it was. */
export function withPosition(geometry: Geometry, at: number[], position: Position): Geometry {
  const replace = (value: unknown, path: number[]): unknown => {
    if (path.length === 0) {
      return position;
    }
    const [head, ...rest] = path;
    const list = value as unknown[];
    return list.map((part, index) => (index === head ? replace(part, rest) : part));
  };
  return { type: geometry.type, coordinates: replace(geometry.coordinates, at) };
}

/** The geometry with one position removed; the ring or line it was in keeps its remaining points. */
export function withoutPosition(geometry: Geometry, at: number[]): Geometry {
  if (at.length === 0) {
    return geometry;
  }
  const drop = (value: unknown, path: number[]): unknown => {
    const list = value as unknown[];
    const [head, ...rest] = path;
    if (rest.length === 0) {
      return list.filter((_, index) => index !== head);
    }
    return list.map((part, index) => (index === head ? drop(part, rest) : part));
  };
  return { type: geometry.type, coordinates: drop(geometry.coordinates, at) };
}

/** The label a row carries, so a screen reader says which point of which ring it reads. */
export function rowLabel(at: number[], point: string = DEFAULT_GEO_LABELS.point): string {
  return at.length === 0 ? point : `${point} ${at.map((index) => index + 1).join(".")}`;
}

interface TerraDrawLike {
  start(): void;
  stop(): void;
  setMode(mode: string): void;
  addFeatures(features: unknown[]): unknown;
  getSnapshot(): Array<{ id?: unknown; geometry: Geometry }>;
  clear(): void;
  undo(): boolean;
  redo(): boolean;
  on(event: string, callback: (...args: unknown[]) => void): void;
}

/** How the editor reaches Terra Draw, injected so a test renders the table without a canvas. */
export interface DrawEngine {
  create(map: MapLibreMap, modes: string[]): TerraDrawLike;
}

/**
 * Terra Draw itself, loaded when the map is ready and not before: jsdom has no canvas, and a server
 * render of a page that merely *contains* an editor should not pull a drawing library in. The
 * library is MIT with no runtime dependencies of its own, and its adapter's peer range covers the
 * `maplibre-gl` this package already installs.
 */
export async function terraDrawEngine(): Promise<DrawEngine> {
  const [terra, adapter] = await Promise.all([
    import("terra-draw"),
    import("terra-draw-maplibre-gl-adapter"),
  ]);
  const byName: Record<string, () => unknown> = {
    point: () => new terra.TerraDrawPointMode(),
    linestring: () => new terra.TerraDrawLineStringMode(),
    polygon: () => new terra.TerraDrawPolygonMode(),
    rectangle: () => new terra.TerraDrawRectangleMode(),
    circle: () => new terra.TerraDrawCircleMode(),
    freehand: () => new terra.TerraDrawFreehandMode(),
    select: () =>
      // Select is what makes an existing shape editable: drag the whole thing, drag a vertex,
      // click a midpoint to insert one, click a vertex to delete it. The same four for every type
      // this editor draws.
      new terra.TerraDrawSelectMode({
        flags: Object.fromEntries(
          ["point", "linestring", "polygon", "rectangle", "circle", "freehand"].map((mode) => [
            mode,
            { feature: { draggable: true, coordinates: { midpoints: true, draggable: true, deletable: true } } },
          ]),
        ),
      }),
  };
  return {
    create(map, modes) {
      const draw = new terra.TerraDraw({
        adapter: new adapter.TerraDrawMapLibreGLAdapter({ map }),
        modes: modes.flatMap((name) => (byName[name] ? [byName[name]()] : [])) as never,
      });
      return draw as unknown as TerraDrawLike;
    },
  };
}

export function GeoEditor({
  value,
  onChange,
  allowed = GEOMETRY_TYPES,
  snapTo,
  basemap,
  engine,
  labels,
  label = "Geometry editor",
}: {
  value: Geometry | null;
  /** The checked geometry, or `null` when the person cleared it. Never called with a bad one. */
  onChange: (geometry: Geometry | null) => void;
  allowed?: readonly GeometryType[];
  /** Shapes drawn behind the edited one, to draw against; read only. */
  snapTo?: GeoFeature[];
  basemap?: string;
  /**
   * The drawing library, injected. Left out, the map draws and the table edits but there is no
   * pointer drawing — which is what a test and a server render get.
   */
  engine?: DrawEngine;
  /** Every visible string, overridden one key at a time; English where a key is left out. */
  labels?: Partial<GeoLabels>;
  label?: string;
}): React.JSX.Element {
  const container = useRef<HTMLDivElement>(null);
  const map = useRef<MapLibreMap | null>(null);
  const draw = useRef<TerraDrawLike | null>(null);
  const [findings, setFindings] = useState<GeoFinding[]>([]);
  const [mode, setMode] = useState<string | null>(null);
  const [pasted, setPasted] = useState("");
  const style = useMemo(() => styleFor(basemap), [basemap]);
  const modes = useMemo(() => modesFor(allowed), [allowed]);
  const rows = useMemo(() => rowsOf(value), [value]);
  const pasteId = useId();
  const say = useMemo(() => ({ ...DEFAULT_GEO_LABELS, ...labels, modes: { ...DEFAULT_GEO_LABELS.modes, ...labels?.modes } }), [labels]);
  const named = (at: number[]): string => rowLabel(at, say.point);

  /** One door out of this component: nothing reaches the host unchecked. */
  const emit = (candidate: Geometry | null): void => {
    if (candidate === null) {
      setFindings([]);
      onChange(null);
      return;
    }
    const { geometry, findings: reasons } = checkGeometry(candidate, allowed);
    setFindings(reasons);
    if (geometry) {
      onChange(geometry);
    }
  };

  const latest = useRef(emit);
  latest.current = emit;
  const allowedNow = useRef(allowed);
  allowedNow.current = allowed;
  const colors = mapColors();
  const snapNow = useRef(snapTo);
  snapNow.current = snapTo;

  useEffect(() => {
    let gone = false;
    let instance: MapLibreMap | null = null;
    void mapWorkerReady().then(() => {
      if (gone || !container.current || map.current) {
        return;
      }
      instance = new MapLibreMap({
        container: container.current,
        style,
        center: [0, 0],
        zoom: 1,
        canvasContextAttributes: { preserveDrawingBuffer: true },
      });
      map.current = instance;
      setMapReady(true);
      // For a person debugging a screenshot and for the browser check, which needs `project` to
      // know where a vertex sits on screen: the map instance, as `MapView` also exposes it.
      (window as unknown as { kitGeoMap?: MapLibreMap }).kitGeoMap = instance;
      instance.on("error", (event) => {
        console.error("kit: map error", event.error?.message ?? event);
      });
      instance.on("load", () => {
        const ready = instance;
        if (!ready) {
          return;
        }
        // The features drawn against (`snapTo`) stay in sight under the tools as plain read-only
        // marks: drawing an area over the points it should catch needs the points (T-6345).
        ready.addSource(REFERENCE, { type: "geojson", data: { type: "FeatureCollection", features: [] } });
        showReference(ready, snapNow.current);
        ready.addLayer({
          id: `${REFERENCE}-lines`,
          type: "line",
          source: REFERENCE,
          filter: ["match", ["geometry-type"], ["LineString", "MultiLineString", "Polygon", "MultiPolygon"], true, false],
          paint: { "line-color": colors.point, "line-width": 1.5, "line-opacity": 0.7 },
        });
        ready.addLayer({
          id: `${REFERENCE}-points`,
          type: "circle",
          source: REFERENCE,
          filter: ["match", ["geometry-type"], ["Point", "MultiPoint"], true, false],
          paint: { "circle-radius": 5, "circle-color": colors.point, "circle-stroke-width": 1, "circle-stroke-color": colors.stroke },
        });
        void (engine ? Promise.resolve(engine) : terraDrawEngine()).then((library) => {
          if (gone) {
            return;
          }
          const drawing = library.create(ready, [...modes.map((m) => m.name), "select"]);
          draw.current = drawing;
          drawing.start();
          // A shape finished or moved is a geometry like any other: the same check, the same door.
          const took = (): void => {
            // The snapshot is not only the drawn shape: select mode adds a point per vertex and per
            // midpoint so they can be grabbed, and those are Points whatever is being edited. The
            // shape is the last feature whose type the host allows — anything else would send a
            // selection handle to `onChange`, which the check then refuses for the wrong reason.
            const shapes = drawing
              .getSnapshot()
              .filter((feature) => allowedNow.current.includes(feature.geometry?.type));
            const drawn = shapes[shapes.length - 1];
            if (drawn) {
              latest.current(drawn.geometry);
            }
          };
          // `finish` ends every edit (a drawn shape, a dragged vertex or shape, a vertex added or
          // deleted). `change` fires on each frame of a drag, and the crossing check over up to
          // MAX_VERTICES points on every frame froze the tab (T-6230).
          drawing.on("finish", took);
        });
      });
    });
    return () => {
      gone = true;
      draw.current?.stop();
      draw.current = null;
      instance?.remove();
      map.current = null;
    };
    // Built once; the value travels in through the table and out through `emit`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const [mapReady, setMapReady] = useState(false);
  const fitted = useRef(false);
  useEffect(() => {
    const bounds = boundsOf([
      ...(value ? [{ type: "Feature" as const, id: "edited", geometry: value }] : []),
      ...(snapTo ?? []),
    ]);
    if (!fitted.current && bounds && map.current?.fitBounds) {
      fitted.current = true;
      map.current.fitBounds(bounds, { padding: 32, maxZoom: 16, duration: 0 });
    }
  }, [value, snapTo, mapReady]);

  useEffect(() => {
    if (map.current) showReference(map.current, snapTo);
  }, [snapTo, mapReady]);

  const choose = (name: string): void => {
    setMode(name);
    draw.current?.setMode(name);
  };

  const editPosition = (row: CoordinateRow, index: 0 | 1, text: string): void => {
    const next = Number(text);
    if (!value || !Number.isFinite(next)) {
      return;
    }
    const position = [...row.position] as Position;
    position[index] = next;
    emit(withPosition(value, row.at, position));
  };

  const take = (text: string): void => {
    // The cap is applied before `JSON.parse`, because the parse is the expensive part: a person who
    // drops a country's boundaries in gets a sentence, not a frozen tab.
    if (text.length > MAX_GEOJSON_BYTES) {
      setFindings([{ path: "", message: say.tooLarge }]);
      return;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(text);
    } catch {
      setFindings([{ path: "", message: say.notJson }]);
      return;
    }
    const { geometry, findings: reasons } = geometryOf(parsed, allowed);
    setFindings(reasons);
    if (geometry) {
      onChange(geometry);
      setPasted("");
    }
  };

  return (
    <div className="geo-editor">
      <div className="geo-toolbar" role="toolbar" aria-label={say.tools}>
        {modes.map((item) => (
          <button
            key={item.name}
            type="button"
            aria-pressed={mode === item.name}
            onClick={() => choose(item.name)}
          >
            {say.modes[item.name] ?? item.label}
          </button>
        ))}
        <button type="button" aria-pressed={mode === "select"} onClick={() => choose("select")}>
          {say.select}
        </button>
        <button type="button" onClick={() => draw.current?.undo()}>
          {say.undo}
        </button>
        <button type="button" onClick={() => draw.current?.redo()}>
          {say.redo}
        </button>
        <button
          type="button"
          onClick={() => {
            draw.current?.clear();
            emit(null);
          }}
        >
          {say.removeShape}
        </button>
      </div>

      <div className="map-wrap">
        <div className="map" ref={container} data-testid="geo-editor-map" role="application" aria-label={label} />
        {!basemap && <span className="map-notice">{NO_BASEMAP}</span>}
      </div>

      <table className="geo-coordinates">
        <caption>
          {value ? `${value.type}, ${rows.length} × ${say.point}` : say.noGeometry}
        </caption>
        <thead>
          <tr>
            <th scope="col">{say.point}</th>
            <th scope="col">{say.longitude}</th>
            <th scope="col">{say.latitude}</th>
            <th scope="col">
              <span className="sr-only">{say.remove}</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.at.join(".") || "0"}>
              <th scope="row">{named(row.at)}</th>
              <td>
                <input
                  type="number"
                  step="any"
                  aria-label={`${named(row.at)} ${say.longitude.toLowerCase()}`}
                  defaultValue={row.position[0]}
                  onBlur={(event) => editPosition(row, 0, event.target.value)}
                />
              </td>
              <td>
                <input
                  type="number"
                  step="any"
                  aria-label={`${named(row.at)} ${say.latitude.toLowerCase()}`}
                  defaultValue={row.position[1]}
                  onBlur={(event) => editPosition(row, 1, event.target.value)}
                />
              </td>
              <td>
                <button
                  type="button"
                  aria-label={`${say.remove} ${named(row.at).toLowerCase()}`}
                  onClick={() => value && emit(withoutPosition(value, row.at))}
                >
                  {say.remove}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <label htmlFor={pasteId}>{say.paste}</label>
      <textarea
        id={pasteId}
        value={pasted}
        onChange={(event) => setPasted(event.target.value)}
        rows={3}
      />
      <button type="button" onClick={() => take(pasted)} disabled={pasted.trim() === ""}>
        {say.take}
      </button>
      <input
        type="file"
        accept=".json,.geojson,application/geo+json,application/json"
        aria-label={say.upload}
        onChange={async (event) => {
          const file = event.target.files?.[0];
          if (file) {
            // The same cap before the read, so a huge file is never pulled into memory either.
            if (file.size > MAX_GEOJSON_BYTES) setFindings([{ path: "", message: say.tooLarge }]);
            else take(await file.text());
          }
        }}
      />

      {findings.length > 0 && (
        <ul className="geo-findings" role="alert">
          {findings.map((finding) => (
            <li key={`${finding.path}:${finding.message}`}>
              {finding.path ? `${finding.path}: ${finding.message}` : finding.message}
            </li>
          ))}
        </ul>
      )}
      <span className="sr-only" data-testid="geo-vertex-count">
        {positionsOf(value?.coordinates).length}
      </span>
    </div>
  );
}
