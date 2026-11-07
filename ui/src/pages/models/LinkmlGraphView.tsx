import { Fragment, useEffect, useId, useMemo, useRef, useState } from "react";
import type { JSX, KeyboardEvent, PointerEvent, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ToolbarPopover } from "../../components/entities/GridToolbar";
import {
  Button,
  Icon,
  Input,
  Menu,
  MenuContent,
  MenuItem,
  MenuTrigger,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { useDiagramEditing } from "./DiagramEditing";
import { download, mermaidErDiagram, svgFile } from "./diagramExport";
import { layered } from "./diagramLayout";
import { foldImports, neighbourhood, nextBox, shortestPath } from "./diagramReading";
import type { Direction } from "./diagramReading";
import type { LayoutEdge, Point } from "./diagramLayout";
import { graphData, parseModel, withImports } from "./linkml";
import type { GraphEdge, GraphNode, GraphRow, Multiplicity } from "./linkml";
import { MAIN_AREA, PERSONAL_AREA } from "./canvasOverlay";
import type { Positions, Served } from "./canvasOverlay";

/**
 * One box's size, the gaps around it and the three text sizes, all in the SVG's own units.
 *
 * The text sizes belong here and not in a class: inside a `viewBox` a font size is user units
 * that the browser scales with the drawing, not pixels on the page, and `BOX.row` is the line
 * height the layout above counts with. A step of the page's type scale would be a different
 * drawing, so they are geometry — written once, beside the geometry that depends on them.
 */
const BOX = {
  width: 240,
  header: 26,
  row: 16,
  padding: 8,
  gapX: 110,
  gapY: 70,
  /** Room on the right of a box for a relationship of a class with itself. */
  loop: 60,
  title: 11,
  slot: 10,
  edge: 9,
  /** The PK/FK tag before a row's name, and the room it takes. */
  tag: 8,
  tagWidth: 18,
};
/**
 * Characters a row's name and its type may each take before they are cut with "…": half a box
 * at the slot size, so the two never run into each other. The whole row is in its tooltip.
 */
const CHARS = 17;
/** Pixels per drawing unit: natural size, and how far the buttons go each way. */
// `readable` is the floor of a fit: below it a slot line is ~6 px and unreadable (T-6090 review:
// helsinki fitted at 58 % on a laptop, 25 % on a phone); a wider drawing scrolls in its frame.
// `overview` is how far the page's diagram may still shrink on open when that shows its whole width:
// at the 60 % floor a phone cut a three-box model by half a box (T-6254 judge).
const ZOOM = { natural: 1, step: 1.25, min: 0.25, max: 4, fitMax: 2, readable: 0.6, overview: 0.5 };
/**
 * The least zoom the full-screen canvas fits to (T-6094): the slot text, 10 units, is 11 px there.
 * The canvas lays its groups out in as many columns as fit at `canvasColumns`, so a model is
 * read at this size or larger and a wider drawing is panned, never shrunk past reading.
 */
const READABLE = 1.1;
/** The most the canvas fits to: a one-class area at 200 % dwarfed its neighbours' tabs. */
const CANVAS_FIT_MAX = 1.25;
const canvasColumns = (frameWidth: number) => Math.max(1, Math.floor((frameWidth / 1.2 + BOX.gapX) / (BOX.width + BOX.gapX)));
/** One object for "no imports", so the drawing is not recomputed on every render. */
const NO_IMPORTS: Record<string, string> = {};
/** How far focus reaches: the class and up to this many lines out. */
const HOPS = [1, 2, 3] as const;
/** The minimap's width in page pixels. */
const MINIMAP = 160;
/** The canvas minimap's height: it sits in the toolbar row, as high as two of its buttons. */
const MINIMAP_ROW = 56;
const ARROWS: readonly string[] = ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"];
/** The opacity of what lies outside a focus or a path: still there to place it, not in the way. */
const FADED = 0.45;
/** An attribute the picked Endpoint does not serve, on a class it does. */
const UNSERVED = 0.35;
/**
 * Header colours by area (T-6094): the model's own classes in the primary tint, personal data in
 * the danger tint, every other area (a LinkML `in_subset`, an import) in the next tint by name.
 */
const AREA_FILLS = ["fill-info-soft", "fill-success-soft", "fill-warning-soft"] as const;
function areaFill(area: string | undefined, others: readonly string[]): string {
  if (area === undefined || area === MAIN_AREA) return "fill-primary-soft";
  if (area === PERSONAL_AREA) return "fill-danger-soft";
  return AREA_FILLS[Math.max(0, others.indexOf(area)) % AREA_FILLS.length];
}

/**
 * The ends a line can have (T-3589), one standard notation: UML's hollow triangle at the parent
 * of `is_a` and of a mixin, ER crow's foot at both ends of a reference and a relationship (the
 * notation of LinkML's `gen-erdiagram`), an open arrow at the enum a field picks from.
 */
const CROW: Record<Multiplicity, string> = { "1": "one", "0..1": "zeroOrOne", "1..*": "oneOrMore", "*": "many" };
type End = "triangle" | "arrow" | (typeof CROW)[Multiplicity];

/** The `<marker>` of every end, drawn tip right at (20, 6); `auto-start-reverse` turns it round at a line's start. */
function Markers({ id }: { id: (end: End) => string }): JSX.Element {
  const marker = (end: End, children: JSX.Element, refX = 20) => (
    <marker
      id={id(end)}
      viewBox="0 0 20 12"
      refX={refX}
      refY="6"
      markerWidth="20"
      markerHeight="12"
      markerUnits="userSpaceOnUse"
      orient="auto-start-reverse"
    >
      {children}
    </marker>
  );
  const ring = (cx: number) => <circle cx={cx} cy={6} r={3.5} className="fill-surface" stroke="currentColor" />;
  return (
    <defs>
      {marker("arrow", <path d="M 11 1 L 20 6 L 11 11 z" fill="currentColor" />)}
      {marker("triangle", <path d="M 6 0 L 20 6 L 6 12 z" className="fill-surface" stroke="currentColor" />)}
      {marker("one", <path d="M 12 0 V 12 M 16 0 V 12" fill="none" stroke="currentColor" />)}
      {marker(
        "zeroOrOne",
        <g fill="none" stroke="currentColor">
          {ring(8)}
          <path d="M 16 0 V 12" />
        </g>,
      )}
      {marker("oneOrMore", <path d="M 8 0 V 12 M 12 6 L 20 0 M 12 6 L 20 12 M 12 6 L 20 6" fill="none" stroke="currentColor" />)}
      {marker(
        "many",
        <g fill="none" stroke="currentColor">
          {ring(6)}
          <path d="M 12 6 L 20 0 M 12 6 L 20 12 M 12 6 L 20 6" />
        </g>,
      )}
    </defs>
  );
}

/** The ends of a line of `edge`'s kind: the start's marker and the end's, as `url(#…)`. */
function ends(edge: GraphEdge, id: (end: End) => string): { start?: string; end?: string } {
  const url = (end: End) => `url(#${id(end)})`;
  if (edge.kind === "is_a" || edge.kind === "mixin") return { end: url("triangle") };
  if (edge.kind === "enum") return { end: url("arrow") };
  return { start: url(CROW[edge.fromMultiplicity ?? "*"]), end: url(CROW[edge.toMultiplicity ?? "*"]) };
}

interface Placed extends GraphNode {
  x: number;
  y: number;
  height: number;
}

/** How many lines a box draws below its title: every row of a class, every value of an enum. */
function lines(node: GraphNode): number {
  return node.rows?.length ?? node.slots.length;
}

/**
 * The edge the layout keeps short: a parent above the class that specialises or mixes it in,
 * a class above what its slots reference, an enum below the classes that pick from it.
 */
export function layoutEdge(edge: GraphEdge): LayoutEdge {
  const parent = edge.kind === "is_a" || edge.kind === "mixin";
  return { from: edge.from, to: edge.to, upper: parent ? edge.to : edge.from, lower: parent ? edge.from : edge.to };
}

/**
 * Where each box sits (T-3589): laid out by its lines, not in a grid. Each connected group of
 * classes is a layered drawing of its own, a class one layer from what it is joined to, and the
 * groups stand side by side, wrapped at three boxes so the drawing fits a laptop card at a
 * readable size (T-4991). `bends` holds, per edge, where its line turns as it passes a layer.
 */
export function place(nodes: GraphNode[], edges: GraphEdge[], columns = 3): { placed: Placed[]; bends: Point[][] } {
  const height = (node: GraphNode) => BOX.header + lines(node) * BOX.row + BOX.padding;
  const layout = layered(
    nodes.map((node) => ({ id: node.name, width: BOX.width, height: height(node) })),
    edges.map(layoutEdge),
    { gapX: BOX.gapX, gapY: BOX.gapY, groupGap: BOX.gapX * 2, maxWidth: columns * (BOX.width + BOX.gapX) },
  );
  // In reading order, top to bottom and left to right: the order Tab walks the boxes in.
  const placed = nodes
    .map((node) => {
      const box = layout.boxes.get(node.name);
      return { ...node, x: box?.x ?? 0, y: box?.y ?? 0, height: height(node) };
    })
    .sort((a, b) => a.y - b.y || a.x - b.x);
  return { placed, bends: layout.bends };
}

/** Where the line from a box's centre towards `towards` leaves the box. */
function border(box: Placed, towards: Point): Point {
  const centre = { x: box.x + BOX.width / 2, y: box.y + box.height / 2 };
  const dx = towards.x - centre.x;
  const dy = towards.y - centre.y;
  if (dx === 0 && dy === 0) return centre;
  const scale = Math.min(
    dx === 0 ? Infinity : BOX.width / 2 / Math.abs(dx),
    dy === 0 ? Infinity : box.height / 2 / Math.abs(dy),
  );
  return { x: centre.x + dx * scale, y: centre.y + dy * scale };
}

/**
 * The line between two boxes through its bends, cut where it leaves each box: the first and
 * last points are on the borders, aimed at the next bend (or the other box's centre).
 */
function route(from: Placed, to: Placed, bends: Point[] = []): Point[] {
  const centre = (box: Placed) => ({ x: box.x + BOX.width / 2, y: box.y + box.height / 2 });
  const start = border(from, bends[0] ?? centre(to));
  const end = border(to, bends[bends.length - 1] ?? centre(from));
  return [start, ...bends, end];
}

/** An SVG path through `points`. */
function pathOf(points: Point[]): string {
  return points.map((p, index) => `${index === 0 ? "M" : "L"} ${p.x} ${p.y}`).join(" ");
}

/** The middle of a route, for its label: the middle of its middle piece. */
function middleOf(points: Point[]): Point {
  const at = Math.max(0, Math.floor((points.length - 1) / 2));
  const a = points[at];
  const b = points[Math.min(points.length - 1, at + 1)];
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

/** A label a little way along the line from `start` towards `end`, and to its side. */
function near(start: Point, end: Point): Point {
  const length = Math.hypot(end.x - start.x, end.y - start.y) || 1;
  const ux = (end.x - start.x) / length;
  const uy = (end.y - start.y) / length;
  return { x: start.x + ux * 16 - uy * 9, y: start.y + uy * 16 + ux * 9 + 3 };
}

/** `text` cut to `chars` characters, with "…" where it was cut. */
function cut(text: string, chars = CHARS): string {
  return text.length > chars ? `${text.slice(0, chars - 1)}…` : text;
}
/**
 * A row's name and type cut to one line: they share `2 * CHARS`, so a short name leaves its room
 * to a long type ("→ Organization 0..1" whole beside "publisher"), and each keeps at least `CHARS`
 * when both are long (T-6094 UI judge: relation types cut with "…").
 */
export function fitRow(name: string, type: string): [string, string] {
  const both = 2 * CHARS;
  const nameChars = Math.max(CHARS, both - type.length);
  const typeChars = Math.max(CHARS, both - Math.min(name.length, nameChars));
  return [cut(name, nameChars), cut(type, typeChars)];
}

/**
 * What a row says its value is: a reference names its class and how many, a list says `[]`, a
 * value object lists the fields it holds inside the entity (T-4829).
 */
function typeText(row: GraphRow, typeWord: (type: string) => string = (type) => type): string {
  if (row.key === "fk") return `→ ${row.type} ${row.multiplicity ?? ""}`.trimEnd();
  const type = row.inline ? `{ ${row.inline.join(", ")} }` : typeWord(row.type);
  return row.multivalued ? `${type}[]` : type;
}

/**
 * The model as a picture (DM-13, T-1111): every class a box with its own slots, and a line for
 * every way one class names another.
 *
 * Clicking a class hands it to the caller, which opens it in the structure view. With
 * `onChange` the drawing also edits (T-3589): it writes the same one string the tree and the YAML
 * write, through the same operations (`DiagramEditing.tsx`); without it, it only reads.
 */
export function LinkmlGraphView({
  source,
  imports = NO_IMPORTS,
  onOpenClass,
  onOpenRelationship,
  onAddRelationship,
  onChange,
  canvas = false,
  positions,
  onMove,
  highlight,
  badges,
  areas,
  only,
  selected,
  toolbarEnd,
  typeWord,
}: {
  source: string;
  /** The sources of the models this one imports, by import name: their classes are drawn too. */
  imports?: Record<string, string>;
  onOpenClass?: (name: string) => void;
  /** A relationship's line was pressed: the class it starts from, whose table lists it. */
  onOpenRelationship?: (from: string) => void;
  /** Read-only drawing: the caller's Add relationship form, on a class of this model. */
  onAddRelationship?: (from: string) => void;
  /**
   * Editing on the drawing: where an edited source goes. Absent for a person who may not change
   * the model, who sees no edit handle at all.
   */
  onChange?: (source: string) => void;
  /**
   * The full-screen canvas (T-6094): the frame fills its parent, the wheel zooms, a box can be
   * dragged (or moved with Shift and an arrow key) and the relationship table is left to the page.
   */
  canvas?: boolean;
  /** Where the person moved boxes: drawn there instead of where the layout put them. */
  positions?: Positions;
  onMove?: (name: string, at: Point) => void;
  /** The classes and attributes the picked Endpoint or Pipeline reaches: everything else fades. */
  highlight?: Served;
  /** A count in each class's corner: how many Endpoints serve it. */
  badges?: ReadonlyMap<string, number>;
  /** Each box's area, which colours its header. */
  areas?: ReadonlyMap<string, string>;
  /** Only these boxes are drawn: one diagram tab. */
  only?: ReadonlySet<string>;
  /** The class the details panel shows, outlined. */
  selected?: string;
  /** Ends the toolbar row: the model page's Open full screen (T-6146). */
  toolbarEnd?: ReactNode;
  /** A type in the words of a person who never wrote LinkML: `integer` as "Whole number" (T-6567). */
  typeWord?: (type: string) => string;
}): JSX.Element {
  const { t } = useTranslation();
  const [zoom, setZoom] = useState(ZOOM.natural);
  const frame = useRef<HTMLDivElement>(null);
  const drawing = useRef<SVGSVGElement>(null);
  // Ids of this drawing's markers: two drawings on one page must not share them.
  const prefix = useId();
  const markerId = (end: End) => `${prefix}-${end}`;
  const drag = useRef<{ x: number; y: number; left: number; top: number } | null>(null);
  const boxes = useRef(new Map<string, SVGGElement>());
  const searchList = useId();
  const [folded, setFolded] = useState<ReadonlySet<string>>(() => new Set());
  const [focus, setFocus] = useState<string>();
  const [hops, setHops] = useState<number>(1);
  const [pathTo, setPathTo] = useState<string>();
  const [query, setQuery] = useState("");
  const [view, setView] = useState({ left: 0, top: 0, width: 0, height: 0, whole: false });
  const editing = useDiagramEditing(source, onChange);
  // A relationship being dragged out of a class: where it started and where the pointer is.
  const [connecting, setConnecting] = useState<{ from: string; to?: Point }>();
  const { name, nodes, edges } = useMemo(() => {
    const model = parseModel(source);
    const parsed = Object.fromEntries(Object.entries(imports).map(([key, text]) => [key, parseModel(text)]));
    return { name: model.name || "model", ...graphData(withImports(model, parsed)) };
  }, [source, imports]);
  // What is drawn: the model with the imports a reader folded each as one box.
  const drawn = useMemo(() => {
    const all = foldImports(nodes, edges, folded);
    if (!only) return all;
    const kept = all.nodes.filter((node) => only.has(node.name));
    const names = new Set(kept.map((node) => node.name));
    return { nodes: kept, edges: all.edges.filter((edge) => names.has(edge.from) && names.has(edge.to)) };
  }, [nodes, edges, folded, only]);
  // The canvas wraps its groups at what its frame holds readably; elsewhere at three boxes.
  const [frameWidth, setFrameWidth] = useState(0);
  const columns = canvas && frameWidth > 0 ? canvasColumns(frameWidth) : 3;
  const laid = useMemo(() => place(drawn.nodes, drawn.edges, columns), [drawn, columns]);
  // A box being dragged on the canvas: where it is now, before it is kept.
  const [moving, setMoving] = useState<{ name: string; at: Point; grip: Point; moved: boolean }>();
  // A moved box is drawn where the person put it; a line to it goes straight, its bends were the layout's.
  const { placed, bends } = useMemo(() => {
    const moved = { ...positions, ...(moving?.moved ? { [moving.name]: moving.at } : {}) };
    if (Object.keys(moved).length === 0) return laid;
    return {
      placed: laid.placed.map((node) => (moved[node.name] ? { ...node, ...moved[node.name] } : node)),
      bends: laid.bends.map((points, index) => {
        const edge = drawn.edges[index];
        return edge && (moved[edge.from] || moved[edge.to]) ? [] : points;
      }),
    };
  }, [laid, positions, moving, drawn]);
  const at = useMemo(() => new Map(placed.map((node) => [node.name, node])), [placed]);
  const areaNames = useMemo(() => [...new Set(areas?.values() ?? [])].filter((area) => area !== MAIN_AREA && area !== PERSONAL_AREA).sort(), [areas]);
  const importNames = useMemo(() => [...new Set(nodes.flatMap((node) => (node.from ? [node.from] : [])))], [nodes]);
  // The frame's visible part, in drawing units, for the minimap.
  const measure = () => {
    const frameAt = frame.current;
    if (frameAt)
      setView({
        left: frameAt.scrollLeft,
        top: frameAt.scrollTop,
        width: frameAt.clientWidth,
        height: frameAt.clientHeight,
        // Nothing to scroll to: the frame shows the whole drawing (jsdom, with no layout, says 0 for all four).
        whole: frameAt.clientWidth > 0 && frameAt.scrollWidth <= frameAt.clientWidth && frameAt.scrollHeight <= frameAt.clientHeight,
      });
  };
  useEffect(measure, [zoom, placed, frameWidth]);
  // On load the drawing fits its card's width, never enlarged nor below readable: a big model ran off the card's
  // right edge at its natural size (T-4991). The Fit button may still enlarge a small one.
  // A frame drawn while hidden (a tab not yet open) has no width: fit once it gets one (T-6090).
  const fitted = useRef(false);
  const startAt = useRef<{ left: number; top: number } | null>(null);
  /** The zoom the last fit set, until it took; and the overlay the canvas last brought into view. */
  const settling = useRef<number | undefined>(undefined);
  const revealed = useRef<Served | undefined>(undefined);
  useEffect(() => {
    const frameAt = frame.current;
    if (!frameAt || typeof ResizeObserver === "undefined") return;
    const watch = new ResizeObserver(() => setFrameWidth(frameAt.clientWidth));
    watch.observe(frameAt);
    return () => watch.disconnect();
  }, []);
  // Another diagram tab is another drawing: it fits again.
  useEffect(() => {
    fitted.current = false;
  }, [only, columns]);
  useEffect(() => {
    const available = (frame.current?.clientWidth ?? 0) - 16;
    if (fitted.current || available <= 0 || placed.length === 0) return;
    fitted.current = true;
    const wide = Math.max(...placed.map((node) => node.x + BOX.width)) + BOX.loop + 8;
    // A phone-narrow canvas opens at one box's width, readable, and is panned; the whole model at
    // a phone's width was a quarter size nobody could read (T-6094 UI judge).
    const narrow = canvas && available < 2 * BOX.width;
    // The page's diagram fits its height too, so a model opens as an overview (owner 2026-10-31: "smaller"):
    // the drawing's whole height (as the svg below sizes it) inside the frame's padding, or the
    // bottom row was cut by the frame's own padding (T-6254 judge: DataService cut).
    const tall = Math.max(...placed.map((node) => node.y + node.height)) + BOX.padding + 8;
    const inner = (frame.current?.clientHeight ?? 0) - 16;
    const high = canvas || inner <= 0 ? Infinity : inner / tall;
    const across = available / (narrow ? BOX.width + 16 : wide);
    const fits = Math.min(canvas ? CANVAS_FIT_MAX : ZOOM.natural, across, high);
    const floor = canvas && !narrow ? READABLE : ZOOM.readable;
    const start = !canvas && fits < floor && across >= ZOOM.overview ? Math.min(across, floor) : Math.max(floor, fits);
    // A drawing wider than its frame at the readable floor opens on its first class, the top-left
    // box, not on the empty corner beside it (T-6090 review: a phone showed one cut box). A canvas
    // overlay brings its own first lit class into view instead (the reveal below).
    const classes = placed.filter((node) => node.kind === "class");
    if (wide * start > available && classes.length > 0 && !(canvas && highlight)) {
      const first = classes.reduce((best, node) => (node.y < best.y || (node.y === best.y && node.x < best.x) ? node : best));
      startAt.current = { left: Math.max(0, (first.x - 8) * start), top: Math.max(0, (first.y - 8) * start) };
    }
    settling.current = start;
    setZoom(start);
  }, [placed, frameWidth, canvas, highlight]);
  // The scroll waits for the render that draws the drawing at its new size.
  useEffect(() => {
    if (!startAt.current) return;
    frame.current?.scrollTo?.(startAt.current);
    startAt.current = null;
  }, [zoom]);
  // The canvas brings the first class an overlay lights into view, once per overlay and after the
  // fit's zoom took: on a phone the frame showed a faded class and none of the lit ones (T-6094 UI judge).
  useEffect(() => {
    const frameAt = frame.current;
    if (!canvas || !frameAt || !highlight || revealed.current === highlight) return;
    if (!fitted.current || settling.current !== zoom) return;
    // The picked class when the overlay lights it (a phone picks in the list, then opens the canvas).
    const lit = placed.filter((node) => node.kind === "class" && highlight.has(node.name));
    const first = lit.find((node) => node.name === selected) ?? lit[0];
    if (!first) return;
    revealed.current = highlight;
    const left = first.x * zoom;
    const top = first.y * zoom;
    const inView =
      left >= frameAt.scrollLeft &&
      top >= frameAt.scrollTop &&
      left + BOX.width * zoom <= frameAt.scrollLeft + frameAt.clientWidth &&
      top + first.height * zoom <= frameAt.scrollTop + frameAt.clientHeight;
    if (!inView) frameAt.scrollTo?.({ left: Math.max(0, left - 8), top: Math.max(0, top - 8) });
  }, [canvas, highlight, placed, zoom, selected]);
  // The wheel zooms on the canvas, and with Ctrl or Cmd anywhere; the point under the pointer stays put.
  const zoomNow = useRef(zoom);
  useEffect(() => {
    zoomNow.current = zoom;
  }, [zoom]);
  const drawsAnything = placed.length > 0;
  useEffect(() => {
    const frameAt = frame.current;
    if (!frameAt || !drawsAnything) return;
    const wheel = (event: WheelEvent) => {
      if (!canvas && !event.ctrlKey && !event.metaKey) return;
      event.preventDefault();
      const current = zoomNow.current;
      const next = Math.min(ZOOM.max, Math.max(ZOOM.min, event.deltaY < 0 ? current * ZOOM.step : current / ZOOM.step));
      if (next === current) return;
      const rect = frameAt.getBoundingClientRect();
      const x = event.clientX - rect.left;
      const y = event.clientY - rect.top;
      const left = (frameAt.scrollLeft + x) * (next / current) - x;
      const top = (frameAt.scrollTop + y) * (next / current) - y;
      setZoom(next);
      requestAnimationFrame(() => frameAt.scrollTo?.({ left, top }));
    };
    frameAt.addEventListener("wheel", wheel, { passive: false });
    return () => frameAt.removeEventListener("wheel", wheel);
  }, [canvas, drawsAnything]);
  // A drag that moved a box is not also a press that opens it.
  const dragged = useRef(false);

  if (!placed.some((node) => node.kind === "class")) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("models.graph.empty")}
      </p>
    );
  }

  const width = Math.max(...placed.map((node) => node.x + BOX.width)) + BOX.loop;
  const height = Math.max(...placed.map((node) => node.y + node.height)) + BOX.padding;
  const exportSvg = () => {
    if (drawing.current) download(svgFile(drawing.current), `${name}.svg`, "image/svg+xml");
  };
  const exportMermaid = () => download(mermaidErDiagram(nodes, edges), `${name}.mmd`, "text/plain");
  // Focus: the class and its neighbourhood stay, the rest fades; a path lights only its own way.
  const names = placed.map((node) => node.name);
  const focused = focus !== undefined && at.has(focus) ? focus : undefined;
  const way = focused && pathTo && at.has(pathTo) ? shortestPath(names, drawn.edges, focused, pathTo) : undefined;
  const found = way && way.nodes.length > 0 ? way : undefined;
  const around = focused ? neighbourhood(names, drawn.edges, focused, hops) : undefined;
  const lit = found ? new Set(found.nodes) : around;
  const litLines = found ? new Set(found.edges) : undefined;
  const faded = (node: string) => (lit !== undefined && !lit.has(node)) || (highlight !== undefined && !highlight.has(node));
  const fadedLine = (edge: GraphEdge, index: number) =>
    litLines ? !litLines.has(index) : faded(edge.from) || faded(edge.to);
  const status = !focused
    ? ""
    : way && !found
      ? t("models.graph.noRoute", { from: focused, to: pathTo ?? "" })
      : found
        ? t("models.graph.routeStatus", { route: found.nodes.join(" → "), count: found.edges.length })
        : t("models.graph.focusStatus", { name: focused, count: (around?.size ?? 1) - 1 });
  // A box in the middle of the frame.
  const reveal = (target: string) => {
    const box = at.get(target);
    const frameAt = frame.current;
    if (!box || !frameAt) return;
    frameAt.scrollTo?.({
      left: Math.max(0, (box.x + BOX.width / 2 + 4) * zoom - frameAt.clientWidth / 2),
      top: Math.max(0, (box.y + box.height / 2 + 4) * zoom - frameAt.clientHeight / 2),
    });
  };
  const find = (text: string) => {
    const wanted = text.trim().toLowerCase();
    if (!wanted) return;
    const match = names.find((n) => n.toLowerCase() === wanted) ?? names.find((n) => n.toLowerCase().includes(wanted));
    if (match) {
      setFocus(match);
      setPathTo(undefined);
      reveal(match);
    }
  };
  // Arrow keys walk the boxes that take focus, to the nearest one that way.
  const walk = (from: string, event: KeyboardEvent<SVGGElement>) => {
    if (!ARROWS.includes(event.key)) return;
    event.preventDefault();
    const stops = placed.filter((node) => node.kind !== "enum").map((node) => ({ ...node, width: BOX.width }));
    const next = nextBox(stops, from, event.key as Direction);
    if (next) boxes.current.get(next.name)?.focus();
  };
  const keep = (node: string) => (element: SVGGElement | null) => {
    if (element) boxes.current.set(node, element);
    else boxes.current.delete(node);
  };
  const toggleFold = (from: string) =>
    setFolded((current) => {
      const next = new Set(current);
      if (!next.delete(from)) next.add(from);
      return next;
    });
  const jump = (point: { x: number; y: number }) => {
    const frameAt = frame.current;
    frameAt?.scrollTo?.({
      left: Math.max(0, point.x * zoom - frameAt.clientWidth / 2),
      top: Math.max(0, point.y * zoom - frameAt.clientHeight / 2),
    });
  };
  // The pointer in drawing units, where the browser can say (jsdom cannot; the drag still works).
  const toDrawing = (event: PointerEvent<SVGSVGElement>): Point | undefined => {
    const matrix = drawing.current?.getScreenCTM?.();
    if (!matrix) return undefined;
    const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse());
    return { x: point.x, y: point.y };
  };
  // Shift and an arrow key move a box on the canvas, 20 units a press.
  const movable = (node: Placed) => canvas && onMove !== undefined && node.kind === "class";
  const nudge = (node: Placed, event: KeyboardEvent<SVGGElement>): boolean => {
    if (!movable(node) || !event.shiftKey || !ARROWS.includes(event.key)) return false;
    event.preventDefault();
    const step = { ArrowUp: [0, -20], ArrowDown: [0, 20], ArrowLeft: [-20, 0], ArrowRight: [20, 0] }[event.key as Direction];
    onMove?.(node.name, { x: Math.max(0, node.x + step[0]), y: Math.max(0, node.y + step[1]) });
    return true;
  };
  // A press on a box drags the box, not the frame.
  const grabBox = (node: Placed, event: PointerEvent<SVGGElement>) => {
    if (!movable(node)) return;
    event.stopPropagation();
    dragged.current = false;
    const pointer = toDrawing(event as unknown as PointerEvent<SVGSVGElement>);
    if (pointer) setMoving({ name: node.name, at: { x: node.x, y: node.y }, grip: { x: pointer.x - node.x, y: pointer.y - node.y }, moved: false });
  };
  const decor = (node: Placed) => ({
    fill: areas && node.kind !== "import" ? areaFill(areas.get(node.name), areaNames) : undefined,
    badge: node.kind === "class" ? badges?.get(node.name) : undefined,
    badgeLabel: t("models.canvas.badge", { count: badges?.get(node.name) ?? 0 }),
  });
  const servedRow = (node: Placed, row: GraphRow) => highlight?.get(node.name)?.has(row.name);
  const rowTip = (row: GraphRow) =>
    [
      t(`models.graph.row.${row.key ?? "plain"}`, { name: row.name, type: typeText(row, typeWord) }),
      row.required ? t("models.canvas.required") : "",
      row.personal ? t("models.canvas.personal") : "",
    ]
      .filter(Boolean)
      .join("; ");
  const addFrom = editing ? (from: string) => editing.addRelationship(from) : onAddRelationship;
  const stroke = (edge: GraphEdge) =>
    edge.kind === "mixin" ? "6 4" : edge.kind === "range" ? "2 3" : edge.kind === "enum" ? "1 4" : undefined;
  const pairs = drawn.edges.filter((edge) => edge.kind === "relationship");
  const local = new Set(placed.filter((node) => node.kind === "class" && !node.from).map((node) => node.name));

  const zoomTo = (next: number) => setZoom(Math.min(ZOOM.max, Math.max(ZOOM.min, next)));
  // Fit: the whole drawing in the frame's width, never blown up past twice its size nor shrunk
  // below readable; Zoom out still goes further for an overview.
  const fit = () => {
    const available = (frame.current?.clientWidth ?? 0) - 16;
    const narrow = canvas && available < 2 * BOX.width;
    const fits = Math.min(canvas ? CANVAS_FIT_MAX : ZOOM.fitMax, available / (narrow ? BOX.width + 16 : width + 8));
    setZoom(available > 0 ? Math.max(canvas && !narrow ? READABLE : ZOOM.readable, fits) : ZOOM.natural);
    frame.current?.scrollTo?.({ left: 0, top: 0 });
  };
  // Pan by dragging the background; a press on a box or a line is still a click.
  const grab = (event: PointerEvent<HTMLDivElement>) => {
    const at = frame.current;
    if (at) drag.current = { x: event.clientX, y: event.clientY, left: at.scrollLeft, top: at.scrollTop };
  };
  const pan = (event: PointerEvent<HTMLDivElement>) => {
    const from = drag.current;
    const at = frame.current;
    if (!from || !at || event.buttons === 0) return;
    at.scrollLeft = from.left - (event.clientX - from.x);
    at.scrollTop = from.top - (event.clientY - from.y);
  };

  return (
    <div className={canvas ? "flex h-full min-h-0 flex-col gap-2" : "flex flex-col gap-2"}>
      {/* One row of tools above the drawing (T-6090): the notation and the downloads sit in a
          popover and a menu, so the drawing starts right under the row, not below a page of prose. */}
      <div role="toolbar" aria-label={t("models.graph.tools")} className="flex flex-wrap items-center gap-2">
        <Input
          type="search"
          list={searchList}
          value={query}
          aria-label={t("models.graph.find")}
          placeholder={t("models.graph.findHint")}
          className="w-full sm:w-72"
          onChange={(event) => {
            setQuery(event.target.value);
            // A name picked from the list is a choice, not a keystroke: focus it at once.
            if (names.includes(event.target.value)) find(event.target.value);
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              find(query);
            }
          }}
        />
        <datalist id={searchList}>
          {names.map((option) => (
            <option key={option} value={option} />
          ))}
        </datalist>
        <Button size="sm" variant="secondary" onClick={() => zoomTo(zoom / ZOOM.step)} disabled={zoom <= ZOOM.min} disabledReason={t("models.graph.zoomMin")}>
          {/* A phone shows the sign alone, so the toolbar keeps to two rows (T-6090 review). */}
          <span aria-hidden="true" className="sm:hidden">
            −
          </span>
          <span className="max-sm:sr-only">{t("models.graph.zoomOut")}</span>
        </Button>
        <Button size="sm" variant="secondary" onClick={() => zoomTo(zoom * ZOOM.step)} disabled={zoom >= ZOOM.max} disabledReason={t("models.graph.zoomMax")}>
          <span aria-hidden="true" className="sm:hidden">
            +
          </span>
          <span className="max-sm:sr-only">{t("models.graph.zoomIn")}</span>
        </Button>
        <Button size="sm" variant="secondary" onClick={fit}>
          {t("models.graph.fit")}
        </Button>
        <span className="text-caption text-fg-muted max-sm:sr-only" aria-live="polite">
          {t("models.graph.zoomLevel", { percent: Math.round(zoom * 100) })}
        </span>
        <ToolbarPopover label={t("models.graph.notation.button")} end tall>
          <div className="flex flex-col gap-2">
            <Legend id={markerId} />
            <p className="text-caption text-fg-muted">{t("models.graph.legend")}</p>
            {pairs.length > 0 ? <p className="text-caption text-fg-muted">{t("models.graph.legendRelationship")}</p> : null}
          </div>
        </ToolbarPopover>
        <Menu>
          <MenuTrigger asChild>
            <Button size="sm" variant="ghost">
              {t("models.graph.download")}
              <Icon name="chevronDown" className="size-3.5 text-fg-subtle" />
            </Button>
          </MenuTrigger>
          <MenuContent align="end">
            <MenuItem onSelect={exportSvg}>{t("models.graph.exportSvg")}</MenuItem>
            <MenuItem onSelect={exportMermaid}>{t("models.graph.exportMermaid")}</MenuItem>
          </MenuContent>
        </Menu>
        {editing ? (
          <>
            <Button size="sm" onClick={editing.addClass}>
              {t("models.graph.edit.addClass")}
            </Button>
            <Button size="sm" variant="secondary" onClick={editing.undo} disabled={!editing.canUndo} disabledReason={t("app.nothingToUndo")}>
              {t("models.graph.edit.undo")}
            </Button>
          </>
        ) : null}
        {canvas ? (
          // On the canvas the map ends the toolbar row: in the drawing's corner it hid lines (T-6094 UI judge).
          <div className="ml-auto max-sm:hidden">
            <Minimap placed={placed} width={width + 8} height={height + 8} view={view} zoom={zoom} faded={faded} onJump={jump} tall={MINIMAP_ROW} />
          </div>
        ) : null}
        {toolbarEnd ? <div className="ml-auto">{toolbarEnd}</div> : null}
      </div>
      {editing ? <p className="text-caption text-fg-muted">{t("models.graph.edit.hint")}</p> : null}
      <div className="flex flex-wrap items-end gap-2 empty:hidden">
        {focused ? (
          <>
            <label className="flex flex-col gap-1 text-caption text-fg-muted">
              {t("models.graph.hops")}
              <Select value={String(hops)} onChange={(event) => setHops(Number(event.target.value))} className="w-36">
                {HOPS.map((count) => (
                  <option key={count} value={count}>
                    {t("models.graph.hopsOption", { count })}
                  </option>
                ))}
              </Select>
            </label>
            <label className="flex flex-col gap-1 text-caption text-fg-muted">
              {t("models.graph.pathTo")}
              <Select value={pathTo ?? ""} onChange={(event) => setPathTo(event.target.value || undefined)} className="w-56">
                <option value="">{t("models.graph.noPath")}</option>
                {names
                  .filter((option) => option !== focused)
                  .map((option) => (
                    <option key={option} value={option}>
                      {option}
                    </option>
                  ))}
              </Select>
            </label>
            <Button
              size="sm"
              variant="secondary"
              onClick={() => {
                setFocus(undefined);
                setPathTo(undefined);
                setQuery("");
              }}
            >
              {t("models.graph.showAll")}
            </Button>
          </>
        ) : null}
        {importNames.map((from) => (
          <Button key={from} size="sm" variant="secondary" aria-pressed={folded.has(from)} onClick={() => toggleFold(from)}>
            {t("models.graph.foldImport", { name: from })}
          </Button>
        ))}
      </div>
      <p role="status" aria-live="polite" className="text-caption text-fg-muted">
        {status}
      </p>
      <div className={canvas ? "flex min-h-0 flex-1 flex-col" : "flex flex-col gap-3"}>
        <div className={canvas ? "relative min-h-0 min-w-0 flex-1" : "relative min-w-0"}>
        {/* The map rides the drawing's corner, where it costs no row of its own (T-6090). */}
        {canvas ? null : (
          <div className="absolute right-2 top-2 z-10 max-sm:hidden">
            <Minimap placed={placed} width={width + 8} height={height + 8} view={view} zoom={zoom} faded={faded} onJump={jump} />
          </div>
        )}
        <div
          ref={frame}
          className={`${canvas ? "h-full" : "max-h-180"} cursor-grab overflow-auto rounded border border-border bg-surface p-2 active:cursor-grabbing`}
          onPointerDown={grab}
          onPointerMove={pan}
          onPointerUp={() => (drag.current = null)}
          onPointerLeave={() => (drag.current = null)}
          onScroll={measure}
        >
          {/* `group`, not `img`: an image's contents are presentational, so the class boxes —
              which are focusable buttons — were reachable by Tab and invisible to the screen
              reader that had just been told this was one picture. */}
          <svg
            ref={drawing}
            role="group"
            aria-label={t("models.graph.title")}
            viewBox={`-4 -4 ${width + 8} ${height + 8}`}
            width={(width + 8) * zoom}
            height={(height + 8) * zoom}
            // Centred in a wider frame: an overview opened in the left third with the rest empty (T-6254).
            className="mx-auto block min-h-48"
            onPointerMove={(event) => {
              if (connecting) setConnecting({ from: connecting.from, to: toDrawing(event) });
              const pointer = moving ? toDrawing(event) : undefined;
              if (moving && pointer) {
                const at = { x: Math.max(0, pointer.x - moving.grip.x), y: Math.max(0, pointer.y - moving.grip.y) };
                const moved = moving.moved || Math.hypot(at.x - moving.at.x, at.y - moving.at.y) > 3;
                setMoving({ ...moving, at: moved ? at : moving.at, moved });
              }
            }}
            onPointerUp={() => {
              setConnecting(undefined);
              if (moving?.moved) {
                dragged.current = true;
                onMove?.(moving.name, moving.at);
              }
              setMoving(undefined);
            }}
          >
            <Markers id={markerId} />
            {connecting?.to && at.get(connecting.from) ? (
              <path
                data-testid="diagram-connecting"
                d={pathOf([
                  { x: (at.get(connecting.from)?.x ?? 0) + BOX.width, y: (at.get(connecting.from)?.y ?? 0) + (at.get(connecting.from)?.height ?? 0) / 2 },
                  connecting.to,
                ])}
                className="stroke-primary-soft-fg"
                strokeWidth={1.5}
                strokeDasharray="4 3"
                fill="none"
              />
            ) : null}

            {drawn.edges.map((edge, index) => {
              const from = at.get(edge.from);
              const to = at.get(edge.to);
              if (!from || !to) {
                return null;
              }
              if (edge.kind === "relationship") {
                return (
                  <RelationshipLine
                    key={`relationship-${edge.from}-${edge.label ?? ""}`}
                    edge={edge}
                    from={from}
                    to={to}
                    bends={bends[index]}
                    markers={ends(edge, markerId)}
                    faded={fadedLine(edge, index)}
                    strong={litLines?.has(index) === true}
                    label={t("models.graph.openRelationship", {
                      name: edge.label ?? "",
                      inverse: edge.inverse ?? "",
                      source: edge.from,
                      target: edge.to,
                    })}
                    onOpen={() =>
                      editing && local.has(edge.from) && local.has(edge.to)
                        ? editing.editRelationship(edge)
                        : onOpenRelationship?.(edge.from)
                    }
                  />
                );
              }
              const points = route(from, to, bends[index]);
              const first = points[0];
              const last = points[points.length - 1];
              const label = middleOf(points);
              return (
                <g
                  key={`${edge.kind}-${edge.from}-${edge.to}-${edge.label ?? ""}`}
                  className="text-fg-subtle"
                  opacity={fadedLine(edge, index) ? FADED : undefined}
                >
                  <path
                    d={pathOf(points)}
                    fill="none"
                    stroke="currentColor"
                    strokeWidth={litLines?.has(index) ? 2.5 : 1}
                    strokeDasharray={stroke(edge)}
                    markerStart={ends(edge, markerId).start}
                    markerEnd={ends(edge, markerId).end}
                  />
                  {edge.label ? (
                    <text
                      x={label.x}
                      y={label.y - 2}
                      textAnchor="middle"
                      fontSize={BOX.edge}
                      className="fill-current"
                    >
                      {edge.label}
                    </text>
                  ) : null}
                  {/* A reference says how many at both ends too (T-2881): its own slot's at the
                      target, and at the source `*`, since no inverse limits it. */}
                  {edge.toMultiplicity ? (
                    <>
                      <Multiplicity at={near(first, points[1])} value={edge.fromMultiplicity} />
                      <Multiplicity at={near(last, points[points.length - 2])} value={edge.toMultiplicity} />
                    </>
                  ) : null}
                </g>
              );
            })}

            {placed.map((node) => {
              const box = (
                <>
                  <rect
                    x={node.x}
                    y={node.y}
                    width={BOX.width}
                    height={node.height}
                    rx={node.kind === "enum" ? 2 : 6}
                    {...outline(node, node.name === selected)}
                  />
                  <BoxDecor node={node} {...decor(node)} />
                  <text
                    x={node.x + BOX.padding}
                    y={node.y + 17}
                    fontSize={BOX.title}
                    className="fill-current font-semibold"
                  >
                    {node.kind === "enum" ? `«enum» ${node.name}` : node.name}
                  </text>
                  {node.from && node.kind !== "import" ? (
                    <text
                      x={node.x + BOX.width - BOX.padding}
                      y={node.y + 17}
                      fontSize={BOX.edge}
                      textAnchor="end"
                      className="fill-current text-fg-muted"
                    >
                      {t("models.graph.from", { name: node.from })}
                    </text>
                  ) : null}
                  {node.rows
                    ? node.rows.map((row, index) => (
                        <SlotRow
                          key={row.name}
                          row={row}
                          served={servedRow(node, row)}
                          x={node.x}
                          y={node.y + BOX.header + index * BOX.row + 4}
                          tip={rowTip(row)}
                          typeWord={typeWord}
                        />
                      ))
                    : node.slots.map((slot, index) => (
                        <text
                          key={slot}
                          x={node.x + BOX.padding}
                          y={node.y + BOX.header + index * BOX.row + 4}
                          fontSize={BOX.slot}
                          className="fill-current text-fg-muted"
                        >
                          {slot}
                        </text>
                      ))}
                </>
              );
              // The drawing's one edit, on a class of this model: its inverse is written here too.
              const add =
                addFrom && node.kind === "class" && local.has(node.name) ? (
                  <g
                    key={`add-${node.name}`}
                    role="button"
                    tabIndex={0}
                    aria-label={t("models.graph.addRelationship", { name: node.name })}
                    className="focus-ring cursor-pointer"
                    onClick={() => addFrom(node.name)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        addFrom(node.name);
                      }
                    }}
                  >
                    <rect
                      x={node.x + BOX.width - 22}
                      y={node.y + node.height - 20}
                      width={16}
                      height={16}
                      rx={3}
                      className="fill-surface stroke-border"
                      strokeWidth={1}
                    />
                    <text
                      x={node.x + BOX.width - 14}
                      y={node.y + node.height - 8}
                      fontSize={BOX.title}
                      textAnchor="middle"
                      className="fill-current"
                    >
                      +
                    </text>
                  </g>
                ) : null;
              // Edit handles, on a class of this model only: an imported one is another model's.
              const handles =
                editing && node.kind === "class" && local.has(node.name) ? (
                  <>
                    <g
                      role="button"
                      tabIndex={0}
                      aria-label={t("models.graph.edit.addField", { name: node.name })}
                      className="focus-ring cursor-pointer"
                      onClick={() => editing.addField(node.name)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter" || event.key === " ") {
                          event.preventDefault();
                          editing.addField(node.name);
                        }
                      }}
                    >
                      <rect
                        x={node.x + BOX.width - 70}
                        y={node.y + node.height - 20}
                        width={44}
                        height={16}
                        rx={3}
                        className="fill-surface stroke-border"
                        strokeWidth={1}
                      />
                      <text
                        x={node.x + BOX.width - 48}
                        y={node.y + node.height - 9}
                        fontSize={BOX.edge}
                        textAnchor="middle"
                        className="fill-current"
                      >
                        {t("models.graph.edit.fieldButton")}
                      </text>
                    </g>
                    {/* The drag handle: pointer only; the "+" is the same form for a keyboard. */}
                    <circle
                      data-testid={`diagram-connect-${node.name}`}
                      aria-hidden="true"
                      cx={node.x + BOX.width}
                      cy={node.y + node.height / 2}
                      r={6}
                      className="cursor-crosshair fill-surface stroke-primary-soft-fg"
                      strokeWidth={1.5}
                      onPointerDown={(event) => {
                        // Not a pan of the frame: this press draws a line.
                        event.stopPropagation();
                        setConnecting({ from: node.name });
                      }}
                    />
                  </>
                ) : null;
              // An enum opens nothing, so it is not a button: a stop in the tab order that does
              // nothing when pressed is worse than none. It says what it is to a screen reader.
              const dim = faded(node.name) ? FADED : undefined;
              if (node.kind === "enum") {
                return (
                  <g
                    key={`enum-${node.name}`}
                    role="img"
                    opacity={dim}
                    aria-label={t("models.graph.enumBox", { name: node.name, values: node.slots.join(", ") })}
                  >
                    {box}
                  </g>
                );
              }
              // A folded import is one box; pressing it unfolds it.
              const open = node.kind === "import" ? () => toggleFold(node.from ?? "") : () => onOpenClass?.(node.name);
              return (
                <Fragment key={node.name}>
                  <g
                    ref={keep(node.name)}
                    role="button"
                    tabIndex={0}
                    opacity={dim}
                    aria-label={
                      node.kind === "import"
                        ? t("models.graph.unfoldBox", { name: node.from ?? "", names: node.slots.join(", ") })
                        : t("models.graph.openClass", { name: node.name })
                    }
                    // `focus-ring`, never a bare `outline-none`: the box is in the tab order, so
                    // taking its outline away left a keyboard with nothing to follow (UI-15).
                    className="focus-ring cursor-pointer"
                    onClick={() => {
                      if (dragged.current) dragged.current = false;
                      else open();
                    }}
                    onPointerDown={(event) => grabBox(node, event)}
                    onPointerUp={() => {
                      // A drag that ends on a class of this model opens the form on that pair.
                      if (editing && connecting && node.kind === "class" && local.has(node.name)) {
                        editing.addRelationship(connecting.from, node.name);
                        setConnecting(undefined);
                      }
                    }}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        open();
                      }
                      if (!nudge(node, event)) walk(node.name, event);
                    }}
                  >
                    {box}
                  </g>
                  {add}
                  {handles}
                </Fragment>
              );
            })}
          </svg>
        </div>
        </div>
        {editing?.dialogs}
        {pairs.length > 0 && !canvas ? (
          <Table caption={t("models.graph.table")}>
            <TableHead>
              <TableHeaderCell>{t("models.graph.tableRelationship")}</TableHeaderCell>
              <TableHeaderCell>{t("models.graph.tableField")}</TableHeaderCell>
              <TableHeaderCell>{t("models.graph.tableInverse")}</TableHeaderCell>
              <TableHeaderCell>{t("models.graph.tableMultiplicity")}</TableHeaderCell>
            </TableHead>
            <TableBody>
              {pairs.map((edge) => (
                <TableRow key={`${edge.from}.${edge.label ?? ""}`}>
                  <TableCell primary>
                    {t(`models.relationships.sentence.${edge.cardinality ?? "one-to-one"}`, { source: edge.from, target: edge.to })}
                  </TableCell>
                  <TableCell>{`${edge.from}.${edge.label ?? ""}`}</TableCell>
                  <TableCell>{`${edge.to}.${edge.inverse ?? ""}`}</TableCell>
                  <TableCell>{`${edge.from} ${edge.fromMultiplicity ?? ""} — ${edge.toMultiplicity ?? ""} ${edge.to}`}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        ) : null}
      </div>
    </div>
  );
}

/** A box's border: the picked class outlined in the primary colour, an import or enum dashed. */
function outline(node: Placed, picked: boolean): { className: string; strokeWidth: number; strokeDasharray?: string } {
  if (picked) return { className: "fill-surface-subtle stroke-primary-soft-fg", strokeWidth: 2.5 };
  return {
    className: node.kind === "enum" ? "fill-surface stroke-border" : "fill-surface-subtle stroke-border",
    strokeWidth: 1,
    strokeDasharray: node.from || node.kind !== "class" ? "4 3" : undefined,
  };
}

/** The header band in its area's tint (square below, so it meets the rows) and the Endpoint count. */
function BoxDecor({ node, fill, badge, badgeLabel }: { node: Placed; fill?: string; badge?: number; badgeLabel: string }): JSX.Element {
  const { x, y } = node;
  const right = x + BOX.width;
  return (
    <>
      {fill ? (
        <path
          data-testid={`diagram-area-${node.name}`}
          d={`M${x} ${y + BOX.header} V${y + 6} Q${x} ${y} ${x + 6} ${y} H${right - 6} Q${right} ${y} ${right} ${y + 6} V${y + BOX.header} Z`}
          className={fill}
        />
      ) : null}
      {badge ? (
        <g data-testid={`diagram-badge-${node.name}`}>
          <title>{badgeLabel}</title>
          <circle cx={right - 4} cy={y + 4} r={9} className="fill-primary-soft-fg" />
          <text x={right - 4} y={y + 7.5} textAnchor="middle" fontSize={BOX.edge} className="fill-surface font-semibold">
            {badge}
          </text>
        </g>
      ) : null}
    </>
  );
}

/** How many, at one end of a line: bold, so it is read before the name in the middle. */
function Multiplicity({ at, value }: { at: Point; value?: string }): JSX.Element | null {
  return value ? (
    <text x={at.x} y={at.y} textAnchor="middle" fontSize={BOX.edge} className="fill-current font-semibold">
      {value}
    </text>
  ) : null;
}

/**
 * One row of a class's box (T-2881): a PK or FK tag, told apart by its word and its border (a
 * solid one for the key, a dashed one for a reference) and never by colour, the slot's name, and
 * its type at the right edge. The tooltip says the whole row when either half was cut.
 */
function SlotRow({ row, x, y, tip, served, typeWord }: { row: GraphRow; x: number; y: number; tip: string; served?: boolean; typeWord?: (type: string) => string }): JSX.Element {
  const nameAt = x + BOX.padding + BOX.tagWidth + 4;
  const [nameText, typeShown] = fitRow(row.name, typeText(row, typeWord));
  // Under the Endpoints overlay a served attribute is in the primary colour, the others fade.
  return (
    <g
      data-served={served === undefined ? undefined : String(served)}
      className={served ? "text-primary-soft-fg" : row.key ? "text-fg" : "text-fg-muted"}
      opacity={served === false ? UNSERVED : undefined}
    >
      <title>{tip}</title>
      {row.key ? (
        <>
          <rect
            x={x + BOX.padding}
            y={y - 9}
            width={BOX.tagWidth}
            height={11}
            rx={2}
            fill="none"
            stroke="currentColor"
            strokeWidth={1}
            strokeDasharray={row.key === "fk" ? "2 1" : undefined}
          />
          <text
            x={x + BOX.padding + BOX.tagWidth / 2}
            y={y}
            textAnchor="middle"
            fontSize={BOX.tag}
            className="fill-current font-semibold"
          >
            {row.key === "pk" ? "PK" : "FK"}
          </text>
        </>
      ) : null}
      <text
        x={nameAt}
        y={y + 1}
        fontSize={BOX.slot}
        className={row.key === "pk" ? "fill-current font-semibold" : "fill-current"}
        fontStyle={row.implied ? "italic" : undefined}
      >
        {nameText}
        {row.required ? " *" : ""}
      </text>
      {row.personal && !row.key ? (
        // Personal data (DM-120): a PD tag in the danger colour, where a key's PK/FK would stand.
        <text x={x + BOX.padding + BOX.tagWidth / 2} y={y} textAnchor="middle" fontSize={BOX.tag} className="fill-danger font-semibold">
          PD
        </text>
      ) : null}
      <text x={x + BOX.width - BOX.padding} y={y + 1} textAnchor="end" fontSize={BOX.slot} className="fill-current">
        {typeShown}
      </text>
    </g>
  );
}

/**
 * A relationship as one line (DM-64): the slot and its inverse in the middle, and at each end how
 * many entities of that class one entity at the other end is joined to. A class related to
 * itself is a loop on its right side.
 */
function RelationshipLine({
  edge,
  from,
  to,
  bends,
  markers,
  faded,
  strong,
  label,
  onOpen,
}: {
  edge: GraphEdge;
  from: Placed;
  to: Placed;
  bends?: Point[];
  markers: { start?: string; end?: string };
  /** Outside the focus or the path: drawn faint. */
  faded: boolean;
  /** On the path: drawn bold. */
  strong: boolean;
  label: string;
  onOpen: () => void;
}): JSX.Element {
  const self = from.name === to.name;
  const top = { x: from.x + BOX.width, y: from.y + from.height * 0.3 };
  const bottom = { x: from.x + BOX.width, y: from.y + from.height * 0.7 };
  const points = self ? [top, bottom] : route(from, to, bends);
  const start = points[0];
  const end = points[points.length - 1];
  const path = self
    ? `M ${top.x} ${top.y} C ${top.x + BOX.loop} ${top.y - 20} ${bottom.x + BOX.loop} ${bottom.y + 20} ${bottom.x} ${bottom.y}`
    : pathOf(points);
  const atFrom = self ? { x: top.x + 10, y: top.y - 4 } : near(start, points[1]);
  const atTo = self ? { x: bottom.x + 10, y: bottom.y + 12 } : near(end, points[points.length - 2]);
  const centre = middleOf(points);
  const middle = self ? { x: top.x + BOX.loop * 0.75, y: (top.y + bottom.y) / 2 + 3 } : { x: centre.x, y: centre.y - 4 };
  return (
    <g
      role="button"
      tabIndex={0}
      aria-label={label}
      className="focus-ring cursor-pointer text-fg"
      opacity={faded ? FADED : undefined}
      onClick={onOpen}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onOpen();
        }
      }}
    >
      {/* A wide transparent stroke under the line, so it can be hit with a pointer. */}
      <path d={path} fill="none" stroke="transparent" strokeWidth={12} />
      <path
        d={path}
        fill="none"
        stroke="currentColor"
        strokeWidth={strong ? 3 : 1.5}
        markerStart={markers.start}
        markerEnd={markers.end}
      />
      <text x={middle.x} y={middle.y} textAnchor="middle" fontSize={BOX.edge} className="fill-current">
        {`${edge.label ?? ""} / ${edge.inverse ?? ""}`}
      </text>
      <text x={atFrom.x} y={atFrom.y} textAnchor="middle" fontSize={BOX.edge} className="fill-current font-semibold">
        {edge.fromMultiplicity}
      </text>
      <text x={atTo.x} y={atTo.y} textAnchor="middle" fontSize={BOX.edge} className="fill-current font-semibold">
        {edge.toMultiplicity}
      </text>
    </g>
  );
}

/**
 * What each end of a line means, drawn with the drawing's own markers (T-3589): read before the
 * drawing, so the notation never has to be guessed.
 */
function Legend({ id }: { id: (end: End) => string }): JSX.Element {
  const { t } = useTranslation();
  const caption = useId();
  const sample = (end: End, dash?: string) => (
    <svg aria-hidden="true" width={64} height={14} viewBox="0 0 64 14" className="shrink-0 text-fg">
      <line x1={2} y1={7} x2={42} y2={7} stroke="currentColor" strokeDasharray={dash} markerEnd={`url(#${id(end)})`} />
    </svg>
  );
  const entries: [JSX.Element, string][] = [
    [sample("triangle"), t("models.graph.notation.isA")],
    [sample("triangle", "6 4"), t("models.graph.notation.mixin")],
    [sample("one"), t("models.graph.notation.one")],
    [sample("zeroOrOne"), t("models.graph.notation.zeroOrOne")],
    [sample("oneOrMore"), t("models.graph.notation.oneOrMore")],
    [sample("many"), t("models.graph.notation.many")],
    [sample("arrow", "1 4"), t("models.graph.notation.enum")],
  ];
  return (
    <figure aria-labelledby={caption} className="flex flex-col gap-1">
      <figcaption id={caption} className="text-caption font-semibold text-fg">{t("models.graph.notation.title")}</figcaption>
      <ul className="flex flex-wrap gap-x-4 gap-y-1">
        {entries.map(([drawing, text]) => (
          <li key={text} className="flex items-center gap-2 text-caption text-fg-muted">
            {drawing}
            <span>{text}</span>
          </li>
        ))}
      </ul>
    </figure>
  );
}

/**
 * The whole drawing small (T-3589): every box, faded as in the drawing, and the part the frame
 * shows. Pressing a point scrolls the frame there; the keyboard has the arrow keys and the
 * search, so the minimap is a pointer's shortcut and hidden from assistive technology.
 */
function Minimap({
  placed,
  width,
  height,
  view,
  zoom,
  faded,
  onJump,
  tall = MINIMAP,
}: {
  placed: Placed[];
  width: number;
  height: number;
  view: { left: number; top: number; width: number; height: number; whole?: boolean };
  zoom: number;
  faded: (name: string) => boolean;
  onJump: (point: Point) => void;
  /** The most it may be high: the canvas sets it in its toolbar row, out of the drawing's way. */
  tall?: number;
}): JSX.Element | null {
  // A drawing the frame shows whole has nothing to jump to: its map was one grey block (T-6164).
  if (view.whole) return null;
  // At most MINIMAP square: a tall drawing would otherwise draw a tall map over the toolbar.
  const scale = Math.min(MINIMAP / width, tall / height);
  return (
    <svg
      aria-hidden="true"
      data-testid="diagram-minimap"
      width={width * scale}
      height={Math.max(Math.min(24, tall), height * scale)}
      viewBox={`-4 -4 ${width} ${height}`}
      className="cursor-pointer rounded border border-border bg-surface text-fg-subtle shadow-1"
      onClick={(event) => {
        const area = event.currentTarget.getBoundingClientRect();
        onJump({ x: (event.clientX - area.left) / scale - 4, y: (event.clientY - area.top) / scale - 4 });
      }}
    >
      {placed.map((node) => (
        <rect
          key={node.name}
          x={node.x}
          y={node.y}
          width={BOX.width}
          height={node.height}
          fill="currentColor"
          opacity={faded(node.name) ? FADED : 0.6}
        />
      ))}
      {view.width > 0 ? (
        <rect
          x={view.left / zoom - 4}
          y={view.top / zoom - 4}
          width={view.width / zoom}
          height={view.height / zoom}
          fill="none"
          className="stroke-primary-soft-fg"
          strokeWidth={2 / scale}
        />
      ) : null}
    </svg>
  );
}
