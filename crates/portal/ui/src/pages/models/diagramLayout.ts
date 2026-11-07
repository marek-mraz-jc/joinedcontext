/**
 * A layered (Sugiyama) layout for the model diagram (T-3589): a class sits one layer away from
 * what it is connected to, so a relationship is a short line between neighbours instead of a
 * line across a grid.
 *
 * The four classic steps, small enough to own: break cycles, assign layers by longest path and
 * pull every node down next to its successors, add a waypoint where a line skips layers, then
 * order each layer by the barycentre of its neighbours (alternating sweeps, the order with the
 * fewest crossings kept) and give each node the x its neighbours ask for without overlapping.
 * Each connected group is laid out on its own and the groups are packed in rows, so unrelated
 * classes stand apart as their own group. No layout library: a model on dev has tens of classes
 * (the largest, Helsinki, 15 classes and 6 enums), where the extra quality of network-simplex
 * ranking would not show and a dependency would have to be pinned, audited and shipped.
 */

export interface LayoutNode {
  id: string;
  width: number;
  height: number;
}

/**
 * A line the layout keeps short: drawn from `from` to `to`, laid out with `upper` one layer
 * above `lower` (a parent above its child, a class above what it references).
 */
export interface LayoutEdge {
  from: string;
  to: string;
  upper: string;
  lower: string;
}

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
  layer: number;
  /** The connected group the node belongs to, in packing order. */
  group: number;
}

/** A box's place and size, all a crossing count or an overlap test needs. */
export type Rect = Pick<Box, "x" | "y" | "width" | "height">;

export interface Point {
  x: number;
  y: number;
}

export interface Layout {
  boxes: Map<string, Box>;
  /** The bends of each edge where it skips layers, from `from` to `to`, by the edge's index. */
  bends: Point[][];
  width: number;
  height: number;
}

export interface Spacing {
  gapX: number;
  gapY: number;
  /** Room between two groups. */
  groupGap: number;
  /** The width the rows of groups wrap at; a wider group gets a row of its own. */
  maxWidth: number;
}

export const SPACING: Spacing = { gapX: 60, gapY: 80, groupGap: 120, maxWidth: 2400 };

/** Sweeps of the barycentre ordering; past a handful the order of a small graph stops changing. */
const SWEEPS = 12;
/** Passes of the x placement that pull a node towards its neighbours. */
const PULLS = 8;

interface Graph {
  ids: string[];
  /** The edges as laid out, cycles broken: `upper` before `lower`, no self loops. */
  down: Map<string, string[]>;
  up: Map<string, string[]>;
}

/** The connected groups: the nodes joined by edges in either direction, in the model's order. */
function groups(ids: string[], edges: readonly LayoutEdge[]): string[][] {
  const near = new Map(ids.map((id) => [id, [] as string[]]));
  for (const edge of edges) {
    near.get(edge.upper)?.push(edge.lower);
    near.get(edge.lower)?.push(edge.upper);
  }
  const seen = new Set<string>();
  const out: string[][] = [];
  for (const id of ids) {
    if (seen.has(id)) continue;
    const group: string[] = [];
    const stack = [id];
    seen.add(id);
    while (stack.length > 0) {
      const at = stack.pop() as string;
      group.push(at);
      for (const next of near.get(at) ?? []) {
        if (!seen.has(next)) {
          seen.add(next);
          stack.push(next);
        }
      }
    }
    out.push(group);
  }
  return out;
}

/** The edges of one group as a DAG: self loops dropped, an edge that closes a cycle reversed. */
function acyclic(ids: string[], edges: readonly LayoutEdge[]): Graph {
  const inGroup = new Set(ids);
  const out = new Map(ids.map((id) => [id, [] as string[]]));
  for (const edge of edges) {
    if (edge.upper !== edge.lower && inGroup.has(edge.upper) && inGroup.has(edge.lower)) {
      out.get(edge.upper)?.push(edge.lower);
    }
  }
  // Depth-first, in the model's order: an edge back into the current path closes a cycle.
  const state = new Map<string, "open" | "done">();
  const down = new Map(ids.map((id) => [id, [] as string[]]));
  const visit = (start: string) => {
    const stack: { id: string; next: number }[] = [{ id: start, next: 0 }];
    state.set(start, "open");
    while (stack.length > 0) {
      const top = stack[stack.length - 1];
      const targets = out.get(top.id) ?? [];
      if (top.next >= targets.length) {
        state.set(top.id, "done");
        stack.pop();
        continue;
      }
      const target = targets[top.next];
      top.next += 1;
      const seen = state.get(target);
      if (seen === "open") {
        down.get(target)?.push(top.id);
      } else {
        down.get(top.id)?.push(target);
        if (seen === undefined) {
          state.set(target, "open");
          stack.push({ id: target, next: 0 });
        }
      }
    }
  };
  for (const id of ids) if (!state.has(id)) visit(id);
  const up = new Map(ids.map((id) => [id, [] as string[]]));
  for (const [from, targets] of down) {
    const unique = [...new Set(targets)];
    down.set(from, unique);
    for (const to of unique) up.get(to)?.push(from);
  }
  return { ids, down, up };
}

/**
 * Longest-path layers from the top, then every node without a parent pulled down to just above
 * its nearest child, so a class referenced from far below does not sit alone at the top.
 */
function layers(graph: Graph): Map<string, number> {
  const layer = new Map<string, number>();
  const order = topological(graph);
  for (const id of order) {
    const parents = graph.up.get(id) ?? [];
    layer.set(id, parents.length === 0 ? 0 : Math.max(...parents.map((p) => (layer.get(p) ?? 0) + 1)));
  }
  for (const id of [...order].reverse()) {
    const children = graph.down.get(id) ?? [];
    if ((graph.up.get(id) ?? []).length === 0 && children.length > 0) {
      layer.set(id, Math.min(...children.map((c) => layer.get(c) ?? 0)) - 1);
    }
  }
  return layer;
}

function topological(graph: Graph): string[] {
  const waiting = new Map(graph.ids.map((id) => [id, (graph.up.get(id) ?? []).length]));
  const ready = graph.ids.filter((id) => waiting.get(id) === 0);
  const order: string[] = [];
  while (ready.length > 0) {
    const id = ready.shift() as string;
    order.push(id);
    for (const child of graph.down.get(id) ?? []) {
      const left = (waiting.get(child) ?? 0) - 1;
      waiting.set(child, left);
      if (left === 0) ready.push(child);
    }
  }
  return order;
}

/** A layer's nodes, real and waypoint, and the links between adjacent layers. */
interface Layered {
  rows: string[][];
  /** Links from a node to the nodes one layer below it. */
  below: Map<string, string[]>;
  above: Map<string, string[]>;
  /** The waypoints of each laid-out edge, top to bottom, by `upper→lower`. */
  chains: Map<string, string[]>;
}

/** Every edge that skips layers gets a waypoint on each layer it crosses. */
function withWaypoints(graph: Graph, layer: Map<string, number>): Layered {
  const below = new Map<string, string[]>();
  const above = new Map<string, string[]>();
  const link = (a: string, b: string) => {
    below.set(a, [...(below.get(a) ?? []), b]);
    above.set(b, [...(above.get(b) ?? []), a]);
  };
  const at = new Map(layer);
  const chains = new Map<string, string[]>();
  for (const [from, targets] of graph.down) {
    for (const to of targets) {
      const top = at.get(from) ?? 0;
      const bottom = at.get(to) ?? 0;
      const chain: string[] = [];
      let previous = from;
      for (let step = top + 1; step < bottom; step += 1) {
        const waypoint = `\u0000${from}\u0000${to}\u0000${step}`;
        at.set(waypoint, step);
        chain.push(waypoint);
        link(previous, waypoint);
        previous = waypoint;
      }
      link(previous, to);
      chains.set(`${from}\u0000${to}`, chain);
    }
  }
  const depth = Math.max(0, ...at.values());
  const rows: string[][] = Array.from({ length: depth + 1 }, () => []);
  // The first order: the model's own, waypoints after the node they leave from.
  for (const id of graph.ids) rows[at.get(id) ?? 0].push(id);
  for (const chain of chains.values()) for (const waypoint of chain) rows[at.get(waypoint) ?? 0].push(waypoint);
  return { rows, below, above, chains };
}

/** Crossings between two adjacent layers, counted pair by pair (a layer has tens of nodes). */
function crossingsBetween(upper: string[], lower: string[], below: Map<string, string[]>): number {
  const position = new Map(lower.map((id, index) => [id, index]));
  const ends: [number, number][] = [];
  upper.forEach((id, index) => {
    for (const target of below.get(id) ?? []) ends.push([index, position.get(target) ?? 0]);
  });
  let count = 0;
  for (let i = 0; i < ends.length; i += 1) {
    for (let j = i + 1; j < ends.length; j += 1) {
      const [a1, b1] = ends[i];
      const [a2, b2] = ends[j];
      if ((a1 - a2) * (b1 - b2) < 0) count += 1;
    }
  }
  return count;
}

function totalCrossings(rows: string[][], below: Map<string, string[]>): number {
  let count = 0;
  for (let i = 0; i + 1 < rows.length; i += 1) count += crossingsBetween(rows[i], rows[i + 1], below);
  return count;
}

/** One layer sorted by the mean position of its neighbours in `fixed`; a node with none keeps its place. */
function byBarycentre(row: string[], fixed: string[], neighbours: Map<string, string[]>): string[] {
  const position = new Map(fixed.map((id, index) => [id, index]));
  const keyed = row.map((id, index) => {
    const near = (neighbours.get(id) ?? []).map((n) => position.get(n) ?? 0);
    return { id, key: near.length === 0 ? index : near.reduce((a, b) => a + b, 0) / near.length, index };
  });
  // Stable on ties, so a sweep that changes nothing gives the same order back.
  keyed.sort((a, b) => a.key - b.key || a.index - b.index);
  return keyed.map((entry) => entry.id);
}

function order(layered: Layered): string[][] {
  let rows = layered.rows.map((row) => [...row]);
  let best = rows;
  let fewest = totalCrossings(rows, layered.below);
  for (let sweep = 0; sweep < SWEEPS && fewest > 0; sweep += 1) {
    const next = rows.map((row) => [...row]);
    if (sweep % 2 === 0) {
      for (let i = 1; i < next.length; i += 1) next[i] = byBarycentre(next[i], next[i - 1], layered.above);
    } else {
      for (let i = next.length - 2; i >= 0; i -= 1) next[i] = byBarycentre(next[i], next[i + 1], layered.below);
    }
    rows = next;
    const count = totalCrossings(rows, layered.below);
    if (count < fewest) {
      fewest = count;
      best = rows;
    }
  }
  return best;
}

/**
 * The x of every node: left to right in its layer's order, then pulled towards the centres of
 * its neighbours, a left and a right pass keeping the gap.
 */
function columns(rows: string[][], layered: Layered, widthOf: (id: string) => number, gapX: number): Map<string, number> {
  const left = new Map<string, number>();
  for (const row of rows) {
    let x = 0;
    for (const id of row) {
      left.set(id, x);
      x += widthOf(id) + gapX;
    }
  }
  const centre = (id: string) => (left.get(id) ?? 0) + widthOf(id) / 2;
  for (let pass = 0; pass < PULLS; pass += 1) {
    const neighbours = pass % 2 === 0 ? layered.above : layered.below;
    const sequence = pass % 2 === 0 ? rows : [...rows].reverse();
    for (const row of sequence) {
      const wanted = row.map((id) => {
        const near = neighbours.get(id) ?? [];
        return near.length === 0 ? centre(id) : near.reduce((sum, n) => sum + centre(n), 0) / near.length;
      });
      // As near the wish as the node before allows, once from the left and once from the
      // right; the mean of two placements that keep the gaps keeps them too, and neither side
      // is favoured.
      const fromLeft: number[] = [];
      let floor = -Infinity;
      row.forEach((id, index) => {
        fromLeft[index] = Math.max(floor, wanted[index] - widthOf(id) / 2);
        floor = fromLeft[index] + widthOf(id) + gapX;
      });
      let ceiling = Infinity;
      for (let index = row.length - 1; index >= 0; index -= 1) {
        const id = row[index];
        const fromRight = Math.min(ceiling - widthOf(id), wanted[index] - widthOf(id) / 2);
        left.set(id, (fromLeft[index] + fromRight) / 2);
        ceiling = fromRight - gapX;
      }
    }
  }
  const minimum = Math.min(...left.values());
  for (const [id, x] of left) left.set(id, x - minimum);
  return left;
}

/** Waypoints are points: they take no room beyond the gap a line needs. */
const WAYPOINT_WIDTH = 0;

function isWaypoint(id: string): boolean {
  return id.startsWith("\u0000");
}

/** One group laid out from (0, 0). */
function layoutGroup(
  ids: string[],
  edges: readonly LayoutEdge[],
  size: Map<string, LayoutNode>,
  spacing: Spacing,
): { boxes: Map<string, Omit<Box, "group">>; chains: Map<string, Point[]>; width: number; height: number } {
  const graph = acyclic(ids, edges);
  const layer = layers(graph);
  const layered = withWaypoints(graph, layer);
  const rows = order(layered);
  const widthOf = (id: string) => (isWaypoint(id) ? WAYPOINT_WIDTH : (size.get(id)?.width ?? 0));
  const x = columns(rows, layered, widthOf, spacing.gapX);
  const boxes = new Map<string, Omit<Box, "group">>();
  const points = new Map<string, Point>();
  let top = 0;
  rows.forEach((row, index) => {
    const tallest = Math.max(0, ...row.filter((id) => !isWaypoint(id)).map((id) => size.get(id)?.height ?? 0));
    for (const id of row) {
      if (isWaypoint(id)) {
        points.set(id, { x: x.get(id) ?? 0, y: top + tallest / 2 });
      } else {
        const node = size.get(id);
        boxes.set(id, { x: x.get(id) ?? 0, y: top, width: node?.width ?? 0, height: node?.height ?? 0, layer: index });
      }
    }
    top += tallest + spacing.gapY;
  });
  const chains = new Map<string, Point[]>();
  for (const [key, chain] of layered.chains) chains.set(key, chain.map((id) => points.get(id) ?? { x: 0, y: 0 }));
  const width = Math.max(0, ...[...boxes.values()].map((box) => box.x + box.width), ...[...points.values()].map((p) => p.x));
  return { boxes, chains, width, height: Math.max(0, top - spacing.gapY) };
}

/**
 * The diagram's layout: each connected group laid out in layers, the groups packed left to right
 * in rows no wider than `spacing.maxWidth`, the largest group first and single nodes last.
 */
export function layered(nodes: readonly LayoutNode[], edges: readonly LayoutEdge[], spacing: Spacing = SPACING): Layout {
  const size = new Map(nodes.map((node) => [node.id, node]));
  const known = edges.filter((edge) => size.has(edge.upper) && size.has(edge.lower));
  const parts = groups(
    nodes.map((node) => node.id),
    known,
  )
    .map((ids, index) => ({ ids, index }))
    .sort((a, b) => b.ids.length - a.ids.length || a.index - b.index);
  const boxes = new Map<string, Box>();
  const chainOf = new Map<string, Point[]>();
  let x = 0;
  let y = 0;
  let rowHeight = 0;
  let width = 0;
  parts.forEach(({ ids }, group) => {
    const laid = layoutGroup(ids, known, size, spacing);
    if (x > 0 && x + laid.width > spacing.maxWidth) {
      x = 0;
      y += rowHeight + spacing.groupGap;
      rowHeight = 0;
    }
    for (const [id, box] of laid.boxes) boxes.set(id, { ...box, x: box.x + x, y: box.y + y, group });
    for (const [key, points] of laid.chains) chainOf.set(key, points.map((p) => ({ x: p.x + x, y: p.y + y })));
    width = Math.max(width, x + laid.width);
    rowHeight = Math.max(rowHeight, laid.height);
    x += laid.width + spacing.groupGap;
  });
  const bends = edges.map((edge) => {
    const forward = chainOf.get(`${edge.upper}\u0000${edge.lower}`);
    const backward = chainOf.get(`${edge.lower}\u0000${edge.upper}`);
    const chain = forward ?? (backward ? [...backward].reverse() : []);
    // The chain runs upper to lower; the edge is drawn from `from`.
    return edge.from === edge.upper ? chain : [...chain].reverse();
  });
  return { boxes, bends, width, height: y + rowHeight };
}

/** The straight pieces of every edge: from the centre of `from` through its bends to the centre of `to`. */
export function segments(
  layout: { boxes: ReadonlyMap<string, Rect>; bends: readonly Point[][] },
  edges: readonly LayoutEdge[],
): [Point, Point][] {
  const centre = (id: string): Point | undefined => {
    const box = layout.boxes.get(id);
    return box ? { x: box.x + box.width / 2, y: box.y + box.height / 2 } : undefined;
  };
  const out: [Point, Point][] = [];
  edges.forEach((edge, index) => {
    const a = centre(edge.from);
    const b = centre(edge.to);
    if (!a || !b || edge.from === edge.to) return;
    const path = [a, ...(layout.bends[index] ?? []), b];
    for (let i = 0; i + 1 < path.length; i += 1) out.push([path[i], path[i + 1]]);
  });
  return out;
}

/** How many pairs of edge pieces cross, ends shared by both not counted: the drawing's tangle. */
export function crossings(pieces: readonly [Point, Point][]): number {
  const side = (a: Point, b: Point, c: Point) => Math.sign((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x));
  const same = (p: Point, q: Point) => Math.abs(p.x - q.x) < 1e-6 && Math.abs(p.y - q.y) < 1e-6;
  let count = 0;
  for (let i = 0; i < pieces.length; i += 1) {
    for (let j = i + 1; j < pieces.length; j += 1) {
      const [p1, p2] = pieces[i];
      const [q1, q2] = pieces[j];
      if (same(p1, q1) || same(p1, q2) || same(p2, q1) || same(p2, q2)) continue;
      const d1 = side(q1, q2, p1);
      const d2 = side(q1, q2, p2);
      const d3 = side(p1, p2, q1);
      const d4 = side(p1, p2, q2);
      if (d1 * d2 < 0 && d3 * d4 < 0) count += 1;
    }
  }
  return count;
}

/** Whether two boxes overlap (touching edges do not). */
export function overlaps(a: Rect, b: Rect): boolean {
  return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}
