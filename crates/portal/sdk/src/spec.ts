/**
 * The one file the model writes: what the dashboard shows, over which entity types, with which
 * filters. Mirrors `agents/kit.rs` in the Portal, which is the copy that refuses a bad
 * specification before it reaches a browser (AG-54); this copy is what lets the bundle say
 * exactly what is wrong instead of rendering nothing when it is handed one anyway.
 */

import { parseGridConfig } from "./grid/config";
import type { EntityGridConfig } from "./grid/config";

export type Agg = "count" | "sum" | "avg" | "min" | "max";

export interface Source {
  /** A short name views and filters refer to. */
  name: string;
  /** The NGSI-LD entity type read from the endpoint. */
  type: string;
  /** The attributes the views may use; the request asks for these and no more. */
  attrs: string[];
  /** An NGSI-LD `q` applied on the endpoint, before any filter on screen. */
  q?: string;
  /** Entities read at most; default 1000, ceiling 5000. */
  limit?: number;
}

export type Filter =
  | { kind: "search"; source?: string; label?: string; attrs: string[] }
  | { kind: "select"; source?: string; label?: string; attr: string }
  | { kind: "range"; source?: string; label?: string; attr: string };

export interface StatItem {
  label: string;
  agg: Agg;
  attr?: string;
  unit?: string;
}

/** What every view carries: the source it reads, a heading, and the page it sits on. */
interface Card {
  source?: string;
  title?: string;
  /** Views that name a page share a tab bar; views without one are on every page. */
  page?: string;
}

export type View =
  | (Card & { kind: "stats"; items: StatItem[] })
  | (Card & { kind: "map"; location?: string; label?: string; color?: string })
  | (Card & { kind: "table"; columns: string[]; sort?: { attr: string; dir: "asc" | "desc" } })
  | (Card & { kind: "chart"; type: "bar" | "line" | "pie"; x: string; y: string; agg?: Agg; top?: number })
  | (Card & { kind: "detail" })
  /** A window over the selected entity with one input per field; a save changes the rows on screen. */
  | (Card & { kind: "form"; fields?: string[] })
  /**
   * The entity grid (SDK-30, UI-71): the same component the Portal's explorer renders, configured
   * by the same object. Unlike every other view it reads the endpoint itself — it pages, filters
   * per column, shows an attribute's metadata and its history, and takes a correction where the
   * grant allows one — so `grid` is its configuration and `source`/`type` are never part of it:
   * the app reads through its own endpoint and the view's source names the type.
   */
  | (Card & { kind: "grid"; grid?: Omit<EntityGridConfig, "source" | "type" | "compareWith"> });

export interface Spec {
  title: string;
  subtitle?: string;
  sources: Source[];
  filters?: Filter[];
  views: View[];
  theme?: { accent?: string };
}

export const DEFAULT_LIMIT = 1000;
export const MAX_LIMIT = 5000;
const AGGS: Agg[] = ["count", "sum", "avg", "min", "max"];
const FILTERS = ["search", "select", "range"];
const VIEWS = ["stats", "map", "table", "chart", "detail", "form", "grid"];
const CHARTS = ["bar", "line", "pie"];
/** Two names every entity carries whatever the source asked for. */
const ALWAYS = ["id", "type"];

/** The parsed specification, or every reason it cannot be rendered. */
export function parseSpec(input: unknown): { spec: Spec; errors: [] } | { spec: null; errors: string[] } {
  const errors: string[] = [];
  const at = (path: string, message: string) => errors.push(`${path}: ${message}`);
  if (typeof input !== "object" || input === null || Array.isArray(input)) {
    return { spec: null, errors: ["spec: must be an object"] };
  }
  const raw = input as Record<string, unknown>;
  if (typeof raw.title !== "string" || raw.title.trim() === "") {
    at("title", "must be a non-empty string");
  }
  const sources = Array.isArray(raw.sources) ? (raw.sources as unknown[]) : [];
  if (sources.length === 0) {
    at("sources", "must list at least one entity type");
  }
  const attrsOf = new Map<string, Set<string>>();
  /** The entity type of each source, which a grid view is configured for. */
  const typeOf = new Map<string, string>();
  sources.forEach((source, index) => {
    const s = (source ?? {}) as Record<string, unknown>;
    const path = `sources[${index}]`;
    if (typeof s.name !== "string" || s.name === "") {
      at(`${path}.name`, "must be a non-empty string");
    } else if (attrsOf.has(s.name)) {
      at(`${path}.name`, `'${s.name}' is used twice`);
    }
    if (typeof s.type !== "string" || s.type === "") {
      at(`${path}.type`, "must be an entity type");
    }
    const attrs = Array.isArray(s.attrs) ? s.attrs.filter((a): a is string => typeof a === "string") : [];
    if (attrs.length === 0) {
      at(`${path}.attrs`, "must list at least one attribute");
    }
    if (s.limit !== undefined && (typeof s.limit !== "number" || s.limit < 1 || s.limit > MAX_LIMIT)) {
      at(`${path}.limit`, `must be between 1 and ${MAX_LIMIT}`);
    }
    if (typeof s.name === "string" && !attrsOf.has(s.name)) {
      attrsOf.set(s.name, new Set([...ALWAYS, ...attrs]));
      if (typeof s.type === "string") {
        typeOf.set(s.name, s.type);
      }
    }
  });
  const first = sources[0] && typeof (sources[0] as Record<string, unknown>).name === "string"
    ? ((sources[0] as Record<string, unknown>).name as string)
    : "";
  const resolve = (path: string, name: unknown): Set<string> | null => {
    const key = typeof name === "string" ? name : first;
    const known = attrsOf.get(key);
    if (!known) {
      at(`${path}.source`, `'${String(name ?? first)}' is not a source`);
      return null;
    }
    return known;
  };
  const check = (path: string, known: Set<string> | null, attr: unknown) => {
    if (typeof attr !== "string") {
      at(path, "must name an attribute");
    } else if (known && !known.has(attr)) {
      at(path, `'${attr}' is not among the source's attributes`);
    }
  };

  const filters = raw.filters === undefined ? [] : Array.isArray(raw.filters) ? (raw.filters as unknown[]) : null;
  if (filters === null) {
    at("filters", "must be a list");
  }
  (filters ?? []).forEach((filter, index) => {
    const f = (filter ?? {}) as Record<string, unknown>;
    const path = `filters[${index}]`;
    if (!FILTERS.includes(String(f.kind))) {
      at(`${path}.kind`, `must be one of ${FILTERS.join(", ")}`);
      return;
    }
    const known = resolve(path, f.source);
    if (f.kind === "search") {
      const attrs = Array.isArray(f.attrs) ? f.attrs : [];
      if (attrs.length === 0) {
        at(`${path}.attrs`, "must list the attributes to search");
      }
      attrs.forEach((attr, i) => check(`${path}.attrs[${i}]`, known, attr));
    } else {
      check(`${path}.attr`, known, f.attr);
    }
  });

  const views = Array.isArray(raw.views) ? (raw.views as unknown[]) : [];
  if (views.length === 0) {
    at("views", "must list at least one view");
  }
  views.forEach((view, index) => {
    const v = (view ?? {}) as Record<string, unknown>;
    const path = `views[${index}]`;
    if (!VIEWS.includes(String(v.kind))) {
      at(`${path}.kind`, `must be one of ${VIEWS.join(", ")}`);
      return;
    }
    const known = resolve(path, v.source);
    if (v.page !== undefined && (typeof v.page !== "string" || v.page === "")) {
      at(`${path}.page`, "must be a non-empty string");
    }
    switch (v.kind) {
      case "form": {
        const fields = v.fields === undefined ? [] : Array.isArray(v.fields) ? v.fields : null;
        if (fields === null) {
          at(`${path}.fields`, "must be a list");
        }
        (fields ?? []).forEach((f, i) => check(`${path}.fields[${i}]`, known, f));
        break;
      }
      case "stats": {
        const items = Array.isArray(v.items) ? (v.items as unknown[]) : [];
        if (items.length === 0) {
          at(`${path}.items`, "must list at least one tile");
        }
        items.forEach((item, i) => {
          const it = (item ?? {}) as Record<string, unknown>;
          if (!AGGS.includes(it.agg as Agg)) {
            at(`${path}.items[${i}].agg`, `must be one of ${AGGS.join(", ")}`);
          } else if (it.agg !== "count") {
            check(`${path}.items[${i}].attr`, known, it.attr);
          }
          if (typeof it.label !== "string") {
            at(`${path}.items[${i}].label`, "must be a string");
          }
        });
        break;
      }
      case "map":
        if (v.location !== undefined) check(`${path}.location`, known, v.location);
        if (v.label !== undefined) check(`${path}.label`, known, v.label);
        if (v.color !== undefined) check(`${path}.color`, known, v.color);
        break;
      case "table": {
        const columns = Array.isArray(v.columns) ? v.columns : [];
        if (columns.length === 0) {
          at(`${path}.columns`, "must list at least one column");
        }
        columns.forEach((c, i) => check(`${path}.columns[${i}]`, known, c));
        break;
      }
      case "grid": {
        const config = (v.grid ?? {}) as Record<string, unknown>;
        // The app reads through its own endpoint, whose slug it learns when it runs, and the
        // view's source names the type: a spec that set either could point this grid at another
        // endpoint's data, so both are refused here rather than quietly overwritten. `compareWith`
        // names a second endpoint or space and is refused for the same reason — and
        // `jc_core::kinds::grid::GridConfig` carries no such field, so a spec setting it would be
        // refused by the platform that validates the manifest anyway.
        for (const owned of ["source", "type", "compareWith"]) {
          if (config[owned] !== undefined) {
            at(`${path}.grid.${owned}`, "is the view's own source; leave it out");
          }
        }
        const { findings } = parseGridConfig({
          ...config,
          source: { kind: "endpoint", slug: "the-app" },
          type: typeOf.get(typeof v.source === "string" ? v.source : first) ?? "Entity",
        });
        // One validator for the spec and for the Portal's own grid, so a person writing a spec by
        // hand reads the same findings the editor shows, at the path of their own file.
        findings
          .filter((finding) => !["/source", "/type", "/compareWith"].includes(finding.path))
          .forEach((finding) => at(`${path}.grid${finding.path.replaceAll("/", ".")}`, finding.message));
        break;
      }
      case "chart":
        if (!CHARTS.includes(String(v.type))) {
          at(`${path}.type`, `must be one of ${CHARTS.join(", ")}`);
        }
        check(`${path}.x`, known, v.x);
        check(`${path}.y`, known, v.y);
        if (v.agg !== undefined && !AGGS.includes(v.agg as Agg)) {
          at(`${path}.agg`, `must be one of ${AGGS.join(", ")}`);
        }
        break;
      default:
        break;
    }
  });

  if (errors.length > 0) {
    return { spec: null, errors };
  }
  return { spec: raw as unknown as Spec, errors: [] };
}

/** The source a view or filter reads, the first one when it names none. */
export function sourceOf<T extends { source?: string }>(spec: Spec, item: T): Source {
  return spec.sources.find((s) => s.name === item.source) ?? spec.sources[0];
}

/** The page names in order of first appearance; empty when no view names one. */
export function pagesOf(spec: Spec): string[] {
  return [...new Set(spec.views.map((v) => v.page).filter((p): p is string => typeof p === "string"))];
}
