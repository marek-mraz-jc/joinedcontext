/**
 * The builder's dashboard as choices, the shape the server compiles into a Grafana file
 * (`Board`): what a new one holds, what each panel kind may draw, and a refusal's field in the
 * form's own words.
 */
import type { components } from "../../api/schema";
import { firstFree, nextId } from "./builderGrid";

/** The board as the page holds it: the lists the server defaults to empty are always there. */
export type Board = Omit<components["schemas"]["Board"], "panels" | "variables"> & {
  panels: BoardPanel[];
  variables: BoardVariable[];
};
export type BoardPanel = components["schemas"]["BoardPanel"];
export type BoardVariable = components["schemas"]["BoardVariable"];
export type Filter = components["schemas"]["Filter"];
export type PanelKind = components["schemas"]["PanelKind"];
export type Aggregation = components["schemas"]["Aggregation"];
export type Interval = components["schemas"]["Interval"];
export type Op = components["schemas"]["Op"];
type AttributeOption = components["schemas"]["AttributeOption"];

export const PANEL_KINDS: readonly PanelKind[] = ["timeseries", "stat", "table", "gauge"];
export const AGGREGATIONS: readonly Aggregation[] = ["none", "avg", "min", "max", "sum"];
export const INTERVALS: readonly Interval[] = ["auto", "PT1M", "PT5M", "PT15M", "PT1H", "P1D"];
export const OPS: readonly Op[] = ["eq", "ne", "gt", "ge", "lt", "le"];
/** The default ranges the settings offer: Grafana's quick ranges that end now. */
export const RANGES = ["now-15m", "now-1h", "now-6h", "now-12h", "now-24h", "now-2d", "now-7d", "now-30d", "now-90d", "now-1y"] as const;

/** The size Grafana gives a new panel. */
const NEW_W = 12;
const NEW_H = 8;

/** A board as the server answers it, with its lists filled in. */
export function boardOf(sent: components["schemas"]["Board"]): Board {
  return { ...sent, panels: sent.panels ?? [], variables: sent.variables ?? [] };
}

export function emptyBoard(): Board {
  return { title: "", description: "", from: "now-6h", to: "now", refresh: "", variables: [], panels: [] };
}

/** A new panel of `kind` in the first free spot, reading what the last panel read. */
export function newPanel(board: Board, kind: PanelKind, title: string): BoardPanel {
  const last = board.panels[board.panels.length - 1];
  return {
    id: nextId(board.panels),
    kind,
    title,
    grid: firstFree(board.panels, NEW_W, NEW_H),
    endpoint: last?.endpoint ?? "",
    type: last?.type ?? "",
    attribute: "",
    aggregation: kind === "timeseries" ? "avg" : "none",
    interval: "auto",
  };
}

/** A number the model holds: what a chart, a stat and a gauge draw, and a filter compares. */
export function isNumber(option: AttributeOption): boolean {
  return option.kinds.includes("timeseries");
}

/** A text the model holds: what a variable offers and a filter matches. */
export function isText(option: AttributeOption): boolean {
  return option.kinds.includes("table") && !option.kinds.includes("timeseries") && !option.kinds.includes("map");
}

/** The attributes `kind` can draw: numbers, or anything but a link for a table. */
export function drawable(kind: PanelKind, options: readonly AttributeOption[]): AttributeOption[] {
  return options.filter((option) => (kind === "table" ? option.kinds.includes("table") : isNumber(option)));
}

/** The panel after its kind changes: a table takes no aggregation, a chart wants one. */
export function withKind(panel: BoardPanel, kind: PanelKind): BoardPanel {
  if (kind === "table") return { ...panel, kind, aggregation: "none", interval: "auto" };
  if (kind === "timeseries" && (panel.aggregation ?? "none") === "none") return { ...panel, kind, aggregation: "avg" };
  return { ...panel, kind };
}

/** What a panel still needs before it can be previewed or saved, as locale keys. */
export function missing(panel: BoardPanel): string[] {
  const out: string[] = [];
  if (!panel.title.trim()) out.push("builder.need.title");
  if (!panel.endpoint) out.push("builder.need.endpoint");
  if (!panel.type) out.push("builder.need.type");
  if (!panel.attribute) out.push("builder.need.attribute");
  for (const filter of panel.filters ?? []) {
    if (!filter.attribute || (!filter.variable && !(filter.value ?? "").trim())) {
      out.push("builder.need.filter");
      break;
    }
  }
  return out;
}

type Words = (key: string, values?: Record<string, unknown>) => string;

/**
 * A refusal's field path (`panels[1].filters[0].value`) in the form's words: the panel by its
 * title, then the field's label; never the server's raw path.
 */
export function fieldWords(path: string, board: Board, t: Words): string {
  const panel = /^panels\[(\d+)\]\.?(.*)$/.exec(path);
  if (panel) {
    const title = board.panels[Number(panel[1])]?.title.trim() || t("builder.untitledPanel");
    return t("builder.inPanel", { title, field: leafWords(panel[2], t) });
  }
  const variable = /^variables\[(\d+)\]\.?(.*)$/.exec(path);
  if (variable) {
    const name = board.variables[Number(variable[1])]?.name || t("builder.untitledVariable");
    return t("builder.inVariable", { name, field: leafWords(variable[2], t) });
  }
  return leafWords(path, t);
}

const LEAVES: Record<string, string> = {
  title: "builder.field.title",
  description: "builder.field.description",
  from: "builder.field.from",
  to: "builder.field.from",
  refresh: "builder.field.refresh",
  panels: "builder.field.panels",
  variables: "builder.field.variables",
  board: "builder.field.board",
  body: "builder.field.board",
  name: "builder.field.variableName",
  label: "builder.field.variableLabel",
  endpoint: "builder.field.endpoint",
  type: "builder.field.type",
  attribute: "builder.field.attribute",
  kind: "builder.field.kind",
  aggregation: "builder.field.aggregation",
  interval: "builder.field.interval",
  grid: "builder.field.position",
  id: "builder.field.position",
  unit: "builder.field.unit",
  decimals: "builder.field.decimals",
  thresholds: "builder.field.thresholds",
  filters: "builder.field.filters",
  value: "builder.field.filterValue",
  variable: "builder.field.filterVariable",
  op: "builder.field.filterOp",
};

function leafWords(rest: string, t: Words): string {
  if (!rest) return t("builder.field.panel");
  if (rest.startsWith("filters[")) {
    const leaf = rest.split(".").pop() ?? "";
    return t("builder.field.inFilter", { field: t(LEAVES[leaf] ?? "builder.field.filters") });
  }
  if (rest.startsWith("thresholds")) return t("builder.field.thresholds");
  const leaf = rest.split(".")[0];
  return t(LEAVES[leaf] ?? "builder.field.board");
}

/**
 * A 422's lines as `{path, message}`. The server writes each as `path: message`; a line whose
 * message is the problem's own detail comes as the path alone (problemLines).
 */
export function refusals(lines: readonly string[] | null | undefined, detail = ""): { path: string; message: string }[] {
  return (lines ?? []).map((line) => {
    const at = line.indexOf(": ");
    if (at > 0) return { path: line.slice(0, at), message: line.slice(at + 2) };
    return /^[A-Za-z0-9_.[\]]+$/.test(line) ? { path: line, message: detail } : { path: "board", message: line };
  });
}

/** The id of the form control a refusal's path names, so the page can take the person there. */
export function fieldId(path: string, board: Board): string | undefined {
  const panel = /^panels\[(\d+)\]\.?([a-zA-Z]*)(?:\[(\d+)\]\.?([a-zA-Z]*))?/.exec(path);
  if (panel) {
    const id = board.panels[Number(panel[1])]?.id;
    if (id === undefined) return undefined;
    const leaf = panel[2] || "title";
    if (leaf === "filters" && panel[3] !== undefined) return `builder-filter-${id}-${panel[3]}-${panel[4] || "attribute"}`;
    if (leaf === "grid" || leaf === "id") return `builder-panel-${id}-x`;
    if (leaf === "thresholds" || leaf === "kind") return `builder-panel-${id}-title`;
    return `builder-panel-${id}-${leaf}`;
  }
  const variable = /^variables\[(\d+)\]\.?([a-zA-Z]*)/.exec(path);
  if (variable) return `builder-var-${variable[1]}-${variable[2] || "name"}`;
  const leaf = path.split(/[.[]/)[0];
  return ["title", "description", "from", "refresh"].includes(leaf) ? `builder-${leaf}` : undefined;
}

/** One server line (`panels[1].endpoint: …`) with its path in the form's words. */
export function lineInWords(line: string, board: Board, t: Words): string {
  const [first] = refusals([line]);
  if (!first || first.path === "board" || !/^(panels|variables)\[|^(title|description|from|to|refresh)$/.test(first.path)) return line;
  return first.message ? `${fieldWords(first.path, board, t)}: ${first.message}` : fieldWords(first.path, board, t);
}
