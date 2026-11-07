import type { components } from "../../api/schema";

/** The project's data flow map as the API answers it (T-4862). */
export type FlowMap = components["schemas"]["FlowMap"];
export type FlowNode = components["schemas"]["FlowNode"];
export type FlowEdge = components["schemas"]["FlowEdge"];
export type FlowKind = FlowNode["kind"];

/** Where each kind stands at the least: a source left of its pipeline, a space right of it. */
const BASE: Record<FlowKind, number> = {
  MqttBroker: 0,
  DataSource: 1,
  Pipeline: 2,
  Endpoint: 3,
  ContextSpace: 4,
};

export const CARD_WIDTH = 184;
export const CARD_HEIGHT = 72;
const COLUMN_GAP = 44;
const ROW_GAP = 16;

export interface Placed {
  node: FlowNode;
  column: number;
  x: number;
  y: number;
}

export interface Layout {
  placed: Placed[];
  width: number;
  height: number;
}

/**
 * The column of every node: its kind's place, pushed right of everything that flows into it, so a
 * derived pipeline stands right of the space it reads and its own space further right. An edge
 * back into a node still being placed is a loop (a pipeline writing the space it reads) and does
 * not push.
 */
export function columns(map: FlowMap): Map<string, number> {
  const into = new Map<string, string[]>();
  for (const edge of map.edges) {
    into.set(edge.to, [...(into.get(edge.to) ?? []), edge.from]);
  }
  const kinds = new Map(map.nodes.map((node) => [node.id, node.kind]));
  const rank = new Map<string, number>();
  const placing = new Set<string>();
  const place = (id: string): number => {
    const known = rank.get(id);
    if (known !== undefined) {
      return known;
    }
    placing.add(id);
    const kind = kinds.get(id);
    let column = kind ? BASE[kind] : 0;
    for (const from of into.get(id) ?? []) {
      if (!placing.has(from) && kinds.has(from)) {
        column = Math.max(column, place(from) + 1);
      }
    }
    placing.delete(id);
    rank.set(id, column);
    return column;
  };
  map.nodes.forEach((node) => place(node.id));
  // Columns nobody stands in are closed up, so the map is as narrow as its paths.
  const used = [...new Set(rank.values())].sort((a, b) => a - b);
  return new Map([...rank].map(([id, column]) => [id, used.indexOf(column)]));
}

/** Every node at a position: columns left to right, each column in the order of what feeds it. */
export function layout(map: FlowMap): Layout {
  const column = columns(map);
  const byColumn = new Map<number, FlowNode[]>();
  for (const node of map.nodes) {
    const at = column.get(node.id) ?? 0;
    byColumn.set(at, [...(byColumn.get(at) ?? []), node]);
  }
  const row = new Map<string, number>();
  const placed: Placed[] = [];
  let rows = 0;
  for (const at of [...byColumn.keys()].sort((a, b) => a - b)) {
    const nodes = byColumn.get(at) ?? [];
    // A node sits level with the average of what flows into it, which keeps lines short.
    const level = (node: FlowNode) => {
      const feeding = map.edges.filter((edge) => edge.to === node.id && row.has(edge.from));
      return feeding.length === 0
        ? Number.MAX_SAFE_INTEGER
        : feeding.reduce((sum, edge) => sum + (row.get(edge.from) ?? 0), 0) / feeding.length;
    };
    const ordered = [...nodes].sort((a, b) => level(a) - level(b) || a.name.localeCompare(b.name));
    ordered.forEach((node, index) => {
      row.set(node.id, index);
      placed.push({
        node,
        column: at,
        x: at * (CARD_WIDTH + COLUMN_GAP),
        y: index * (CARD_HEIGHT + ROW_GAP),
      });
    });
    rows = Math.max(rows, ordered.length);
  }
  const width = byColumn.size * (CARD_WIDTH + COLUMN_GAP) - COLUMN_GAP;
  return { placed, width: Math.max(width, 0), height: Math.max(rows * (CARD_HEIGHT + ROW_GAP) - ROW_GAP, 0) };
}

/** The curve of an edge from the right side of one card to the left side of the next. */
export function curve(from: Pick<Placed, "x" | "y">, to: Pick<Placed, "x" | "y">): string {
  const x1 = from.x + CARD_WIDTH;
  const y1 = from.y + CARD_HEIGHT / 2;
  const x2 = to.x;
  const y2 = to.y + CARD_HEIGHT / 2;
  const bend = Math.max(Math.abs(x2 - x1) / 2, 24);
  return `M ${x1} ${y1} C ${x1 + bend} ${y1}, ${x2 - bend} ${y2}, ${x2} ${y2}`;
}

/** The edges whose grant is missing: a path the runner will be refused on. */
export function missingGrants(map: FlowMap, id?: string): FlowEdge[] {
  return map.edges.filter(
    (edge) => edge.grant?.state === "missing" && (id === undefined || edge.from === id || edge.to === id),
  );
}

/** What a node reads from and writes to, for its detail and the phone list. */
export function neighbours(map: FlowMap, id: string): { incoming: FlowEdge[]; outgoing: FlowEdge[] } {
  return {
    incoming: map.edges.filter((edge) => edge.to === id),
    outgoing: map.edges.filter((edge) => edge.from === id),
  };
}

export type FlowGroup = "sources" | "pipelines" | "endpoints" | "spaces";

/** The phone's list: the same nodes, grouped by what they are, in the map's order. */
export function groups(map: FlowMap): { group: FlowGroup; nodes: FlowNode[] }[] {
  const of = (kind: FlowKind): FlowGroup =>
    kind === "MqttBroker" || kind === "DataSource"
      ? "sources"
      : kind === "Pipeline"
        ? "pipelines"
        : kind === "Endpoint"
          ? "endpoints"
          : "spaces";
  const order: FlowGroup[] = ["sources", "pipelines", "endpoints", "spaces"];
  return order
    .map((group) => ({ group, nodes: map.nodes.filter((node) => of(node.kind) === group) }))
    .filter((entry) => entry.nodes.length > 0);
}

/**
 * The Endpoints of the project a new pipeline may write through: this project's own, readable
 * ones. What "Send to a space" offers.
 */
export function targets(map: FlowMap): FlowNode[] {
  return map.nodes.filter((node) => node.kind === "Endpoint" && !node.opaque && node.project === map.project);
}

/** The target URN of a pipeline output through `endpoint` (PF-42): the space segment is text only. */
export function targetUrn(orgDomain: string, endpoint: FlowNode): string {
  return `urn:ngsi-ld:Endpoint:${orgDomain}:${endpoint.space ?? endpoint.project}:${endpoint.name}`;
}
