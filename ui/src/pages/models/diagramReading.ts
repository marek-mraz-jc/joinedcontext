/**
 * Reading a big model diagram (T-3589): the neighbourhood of one class, the shortest path between
 * two, imports folded into one box, and which box an arrow key goes to. Pure, over the graph the
 * drawing already has.
 */
import type { GraphEdge, GraphNode } from "./linkml";

/** Each node's neighbours along any line, either direction; a line to a box that is not drawn is skipped. */
function adjacency(nodes: readonly string[], edges: readonly GraphEdge[]): Map<string, Set<string>> {
  const near = new Map(nodes.map((name) => [name, new Set<string>()]));
  for (const edge of edges) {
    if (edge.from === edge.to) continue;
    near.get(edge.from)?.add(edge.to);
    near.get(edge.to)?.add(edge.from);
  }
  return near;
}

/** The nodes within `hops` lines of `start`, `start` included; empty when `start` is not drawn. */
export function neighbourhood(nodes: readonly string[], edges: readonly GraphEdge[], start: string, hops: number): Set<string> {
  const near = adjacency(nodes, edges);
  if (!near.has(start)) return new Set();
  const seen = new Set([start]);
  let frontier = [start];
  for (let step = 0; step < hops && frontier.length > 0; step += 1) {
    const next: string[] = [];
    for (const name of frontier) {
      for (const other of near.get(name) ?? []) {
        if (!seen.has(other)) {
          seen.add(other);
          next.push(other);
        }
      }
    }
    frontier = next;
  }
  return seen;
}

/**
 * The fewest lines from `from` to `to`, either direction: the nodes on the way, in order, and
 * the indices of the lines taken. Empty when no line joins them.
 */
export function shortestPath(
  nodes: readonly string[],
  edges: readonly GraphEdge[],
  from: string,
  to: string,
): { nodes: string[]; edges: number[] } {
  const known = new Set(nodes);
  if (!known.has(from) || !known.has(to)) return { nodes: [], edges: [] };
  const back = new Map<string, { node: string; edge: number }>();
  const seen = new Set([from]);
  const queue = [from];
  while (queue.length > 0 && !seen.has(to)) {
    const at = queue.shift() as string;
    edges.forEach((edge, index) => {
      const other = edge.from === at ? edge.to : edge.to === at ? edge.from : undefined;
      if (other !== undefined && known.has(other) && !seen.has(other)) {
        seen.add(other);
        back.set(other, { node: at, edge: index });
        queue.push(other);
      }
    });
  }
  if (!seen.has(to)) return { nodes: [], edges: [] };
  const path = [to];
  const taken: number[] = [];
  for (let at = to; at !== from; ) {
    const step = back.get(at) as { node: string; edge: number };
    path.unshift(step.node);
    taken.unshift(step.edge);
    at = step.node;
  }
  return { nodes: path, edges: taken };
}

/** The box a folded import is drawn as. */
export function importBox(name: string): string {
  return `«import» ${name}`;
}

/**
 * The graph with the imports in `folded` each drawn as one box, which lists the names it holds:
 * a line to any of them goes to the box, lines inside one import are dropped, and two lines
 * that become the same line are drawn once.
 */
export function foldImports(
  nodes: readonly GraphNode[],
  edges: readonly GraphEdge[],
  folded: ReadonlySet<string>,
): { nodes: GraphNode[]; edges: GraphEdge[] } {
  if (folded.size === 0) return { nodes: [...nodes], edges: [...edges] };
  const owner = new Map<string, string>();
  const boxes = new Map<string, GraphNode>();
  const kept: GraphNode[] = [];
  for (const node of nodes) {
    if (node.from !== undefined && folded.has(node.from)) {
      const name = importBox(node.from);
      owner.set(node.name, name);
      const box = boxes.get(name) ?? { name, kind: "import", slots: [], depth: 0, from: node.from };
      box.slots.push(node.name);
      if (!boxes.has(name)) {
        boxes.set(name, box);
        kept.push(box);
      }
    } else {
      kept.push(node);
    }
  }
  const seen = new Set<string>();
  const lines: GraphEdge[] = [];
  for (const edge of edges) {
    const from = owner.get(edge.from) ?? edge.from;
    const to = owner.get(edge.to) ?? edge.to;
    if (from === to && from !== edge.from) continue;
    const key = [edge.kind, from, to, edge.label ?? ""].join("\u0000");
    if (seen.has(key)) continue;
    seen.add(key);
    lines.push({ ...edge, from, to });
  }
  return { nodes: kept, edges: lines };
}

export type Direction = "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight";

/**
 * The box an arrow key moves to from `from`: the nearest box whose centre lies in that
 * direction, distance along the arrow counting less than distance across it, so Down goes to
 * the box below rather than a far one diagonally below. Undefined at the edge of the drawing.
 */
export function nextBox<T extends { name: string; x: number; y: number; width: number; height: number }>(
  boxes: readonly T[],
  from: string,
  direction: Direction,
): T | undefined {
  const start = boxes.find((box) => box.name === from);
  if (!start) return undefined;
  const centre = (box: T) => ({ x: box.x + box.width / 2, y: box.y + box.height / 2 });
  const a = centre(start);
  let best: T | undefined;
  let bestScore = Infinity;
  for (const box of boxes) {
    if (box.name === from) continue;
    const b = centre(box);
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const [along, across] =
      direction === "ArrowUp" ? [-dy, dx] : direction === "ArrowDown" ? [dy, dx] : direction === "ArrowLeft" ? [-dx, dy] : [dx, dy];
    if (along <= 0) continue;
    const score = along + 2 * Math.abs(across);
    if (score < bestScore) {
      bestScore = score;
      best = box;
    }
  }
  return best;
}
