import { MAX_FORMULA_CHARS, parseFormula } from "./formula";

export type Density = "compact" | "comfortable";

export interface ColumnShow {
  observedAt?: boolean;
  unit?: boolean;
  datasetId?: boolean;
  createdAt?: boolean;
  modifiedAt?: boolean;
}

export interface GridColumn {
  attr: string;
  label?: string;
  width?: number;
  pinned?: boolean;
  show?: ColumnShow;
  editable?: boolean;
  format?: "text" | "number" | "date" | "link";
  /**
   * A computed column (T-6384): `attr` names it and the formula works its value out of the row's
   * stored attributes, e.g. `availableBikeNumber / totalSlotNumber`. Parsed by `parseFormula`,
   * never evaluated as code; such a column is neither edited nor filtered on the endpoint.
   */
  formula?: string;
}

export type GridSource =
  | { kind: "endpoint"; slug: string }
  | { kind: "space"; space: string }
  | { kind: "fixture"; name: string };

export interface EntityGridConfig {
  source: GridSource;
  type: string;
  columns?: GridColumn[];
  entityTimestamps?: boolean;
  filters?: { allowed?: string[]; preset?: { q?: string; attrs?: string[]; idPattern?: string; scopeQ?: string } };
  pageSize?: number;
  mode?: "view" | "edit";
  editableAttrs?: string[];
  history?: { enabled?: boolean; maxPoints?: number };
  compareWith?: GridSource;
  density?: Density;
  rowActions?: string[];
  /**
   * The map beside the rows (UI-72): a second view of the same page, not a second query. `attr`
   * names the GeoProperty it shows; without one the grid takes the type's first. A type with no
   * GeoProperty gets no map however this reads, because there would be nothing to put on it.
   */
  map?: { enabled?: boolean; attr?: string; position?: "right" | "bottom" };
}

export type ResolvedGridConfig = Required<Omit<EntityGridConfig, "compareWith">> & { compareWith?: GridSource };

export interface ConfigFinding {
  path: string;
  message: string;
}

export const DEFAULT_PAGE_SIZE = 50;
/** A page the grid shows; the source reads it in broker pages of at most `BROKER_PAGE` (owner 2026-11-01: 5000). */
export const MAX_PAGE_SIZE = 5000;

const VALID_FORMATS = new Set(["text", "number", "date", "link"]);
const VALID_MODES = new Set(["view", "edit"]);
const VALID_DENSITIES = new Set(["compact", "comfortable"]);
const VALID_SOURCE_KINDS = new Set(["endpoint", "space", "fixture"]);

function isNonEmptyString(v: unknown): v is string {
  return typeof v === "string" && v.length > 0;
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function checkSource(raw: unknown, path: string, findings: ConfigFinding[]): GridSource | undefined {
  if (!isObject(raw)) {
    findings.push({ path, message: "must be an object" });
    return undefined;
  }
  const kind = raw.kind;
  if (!isNonEmptyString(kind) || !VALID_SOURCE_KINDS.has(kind)) {
    findings.push({ path: `${path}/kind`, message: `must be one of ${[...VALID_SOURCE_KINDS].join(", ")}` });
    return undefined;
  }
  // A source names a slug, a space or a fixture and nothing else: never a URL (EP-55).
  const allowed = ["kind", kind === "endpoint" ? "slug" : kind === "space" ? "space" : "name"];
  for (const key of Object.keys(raw)) {
    if (!allowed.includes(key)) {
      findings.push({ path: `${path}/${key}`, message: "unknown key" });
    }
  }
  if (kind === "endpoint") {
    const slug = raw.slug;
    if (!isNonEmptyString(slug) || slug.includes("/") || slug.includes("?")) {
      findings.push({ path: `${path}/slug`, message: "must be a non-empty string without '/' or '?'" });
      return undefined;
    }
    return { kind: "endpoint", slug };
  }
  if (kind === "space") {
    const space = raw.space;
    if (!isNonEmptyString(space) || space.includes("/") || space.includes("?")) {
      findings.push({ path: `${path}/space`, message: "must be a non-empty string without '/' or '?'" });
      return undefined;
    }
    return { kind: "space", space };
  }
  if (kind === "fixture") {
    const name = raw.name;
    if (!isNonEmptyString(name)) {
      findings.push({ path: `${path}/name`, message: "must be a non-empty string" });
      return undefined;
    }
    return { kind: "fixture", name };
  }
  return undefined;
}

function checkColumns(raw: unknown, path: string, findings: ConfigFinding[]): GridColumn[] | undefined {
  if (!Array.isArray(raw)) {
    findings.push({ path, message: "must be an array" });
    return undefined;
  }
  const columns: GridColumn[] = [];
  for (let i = 0; i < raw.length; i++) {
    const item = raw[i];
    const itemPath = `${path}/${i}`;
    if (!isObject(item)) {
      findings.push({ path: itemPath, message: "must be an object" });
      continue;
    }
    const attr = item.attr;
    if (!isNonEmptyString(attr)) {
      findings.push({ path: `${itemPath}/attr`, message: "must be a non-empty string" });
      continue;
    }
    const col: GridColumn = { attr };
    if (item.label !== undefined) {
      if (isNonEmptyString(item.label)) col.label = item.label;
      else findings.push({ path: `${itemPath}/label`, message: "must be a string" });
    }
    if (item.width !== undefined) {
      if (typeof item.width === "number" && Number.isInteger(item.width) && item.width > 0) col.width = item.width;
      else findings.push({ path: `${itemPath}/width`, message: "must be a positive integer" });
    }
    if (item.pinned !== undefined) {
      if (typeof item.pinned === "boolean") col.pinned = item.pinned;
      else findings.push({ path: `${itemPath}/pinned`, message: "must be a boolean" });
    }
    if (item.show !== undefined) {
      if (isObject(item.show)) {
        const show: ColumnShow = {};
        for (const [k, v] of Object.entries(item.show)) {
          if (!["observedAt", "unit", "datasetId", "createdAt", "modifiedAt"].includes(k)) {
            findings.push({ path: `${itemPath}/show/${k}`, message: "unknown property" });
          } else if (typeof v !== "boolean") {
            findings.push({ path: `${itemPath}/show/${k}`, message: "must be a boolean" });
          } else {
            (show as Record<string, boolean>)[k] = v;
          }
        }
        col.show = show;
      } else {
        findings.push({ path: `${itemPath}/show`, message: "must be an object" });
      }
    }
    if (item.editable !== undefined) {
      if (typeof item.editable === "boolean") col.editable = item.editable;
      else findings.push({ path: `${itemPath}/editable`, message: "must be a boolean" });
    }
    if (item.format !== undefined) {
      if (VALID_FORMATS.has(item.format as string)) col.format = item.format as GridColumn["format"];
      else findings.push({ path: `${itemPath}/format`, message: `must be one of ${[...VALID_FORMATS].join(", ")}` });
    }
    if (item.formula !== undefined) {
      const parsed = typeof item.formula === "string" ? parseFormula(item.formula) : null;
      if (parsed === null) findings.push({ path: `${itemPath}/formula`, message: "must be a string" });
      else if (!parsed.ok) findings.push({ path: `${itemPath}/formula`, message: `${parsed.error} (at character ${parsed.at + 1})` });
      else if (parsed.attrs.includes(attr)) findings.push({ path: `${itemPath}/formula`, message: `reads "${attr}", the column it computes; name the column differently` });
      else if (col.editable === true) findings.push({ path: `${itemPath}/editable`, message: "a computed column is worked out from the row and cannot be edited" });
      else col.formula = item.formula as string;
    }
    // unknown keys in column
    for (const k of Object.keys(item)) {
      if (!["attr", "label", "width", "pinned", "show", "editable", "format", "formula"].includes(k)) {
        findings.push({ path: `${itemPath}/${k}`, message: "unknown property" });
      }
    }
    columns.push(col);
  }
  return columns;
}

export const gridConfigSchema: Record<string, unknown> = {
  $schema: "http://json-schema.org/draft-07/schema#",
  type: "object",
  additionalProperties: false,
  properties: {
    source: {
      oneOf: [
        { type: "object", properties: { kind: { const: "endpoint" }, slug: { type: "string", minLength: 1 } }, required: ["kind", "slug"], additionalProperties: false },
        { type: "object", properties: { kind: { const: "space" }, space: { type: "string", minLength: 1 } }, required: ["kind", "space"], additionalProperties: false },
        { type: "object", properties: { kind: { const: "fixture" }, name: { type: "string", minLength: 1 } }, required: ["kind", "name"], additionalProperties: false },
      ],
    },
    type: { type: "string", minLength: 1 },
    columns: {
      type: "array",
      items: {
        type: "object",
        additionalProperties: false,
        properties: {
          attr: { type: "string", minLength: 1 },
          label: { type: "string" },
          width: { type: "integer", minimum: 1 },
          pinned: { type: "boolean" },
          show: {
            type: "object",
            additionalProperties: false,
            properties: {
              observedAt: { type: "boolean" },
              unit: { type: "boolean" },
              datasetId: { type: "boolean" },
              createdAt: { type: "boolean" },
              modifiedAt: { type: "boolean" },
            },
          },
          editable: { type: "boolean" },
          format: { type: "string", enum: ["text", "number", "date", "link"] },
          formula: { type: "string", minLength: 1, maxLength: MAX_FORMULA_CHARS },
        },
      },
    },
    entityTimestamps: { type: "boolean" },
    filters: {
      type: "object",
      additionalProperties: false,
      properties: {
        allowed: { type: "array", items: { type: "string" } },
        preset: {
          type: "object",
          additionalProperties: false,
          properties: {
            q: { type: "string" },
            attrs: { type: "array", items: { type: "string" } },
            idPattern: { type: "string" },
            scopeQ: { type: "string" },
          },
        },
      },
    },
    pageSize: { type: "integer", minimum: 1, maximum: 5000 },
    mode: { type: "string", enum: ["view", "edit"] },
    editableAttrs: { type: "array", items: { type: "string" } },
    history: {
      type: "object",
      additionalProperties: false,
      properties: {
        enabled: { type: "boolean" },
        maxPoints: { type: "integer", minimum: 1 },
      },
    },
    compareWith: {
      oneOf: [
        { type: "object", properties: { kind: { const: "endpoint" }, slug: { type: "string", minLength: 1 } }, required: ["kind", "slug"], additionalProperties: false },
        { type: "object", properties: { kind: { const: "space" }, space: { type: "string", minLength: 1 } }, required: ["kind", "space"], additionalProperties: false },
        { type: "object", properties: { kind: { const: "fixture" }, name: { type: "string", minLength: 1 } }, required: ["kind", "name"], additionalProperties: false },
      ],
    },
    density: { type: "string", enum: ["compact", "comfortable"] },
    rowActions: { type: "array", items: { type: "string" } },
    map: {
      type: "object",
      additionalProperties: false,
      properties: {
        enabled: { type: "boolean" },
        attr: { type: "string", minLength: 1 },
        position: { enum: ["right", "bottom"] },
      },
    },
  },
  required: ["source", "type"],
};

export function parseGridConfig(raw: unknown): { config?: ResolvedGridConfig; findings: ConfigFinding[] } {
  const findings: ConfigFinding[] = [];

  if (!isObject(raw)) {
    findings.push({ path: "", message: "must be an object" });
    return { findings };
  }

  // unknown top keys
  const knownTop = new Set(["source", "type", "columns", "entityTimestamps", "filters", "pageSize", "mode", "editableAttrs", "history", "compareWith", "density", "rowActions", "map"]);
  for (const k of Object.keys(raw)) {
    if (!knownTop.has(k)) {
      findings.push({ path: `/${k}`, message: "unknown property" });
    }
  }

  const source = checkSource(raw.source, "/source", findings);
  if (!source) {
    // source is required; if missing or invalid we cannot proceed
    return { findings };
  }

  const type = raw.type;
  if (!isNonEmptyString(type)) {
    findings.push({ path: "/type", message: "must be a non-empty string" });
  }

  const columns = raw.columns !== undefined ? checkColumns(raw.columns, "/columns", findings) : undefined;

  const entityTimestamps = raw.entityTimestamps;
  if (entityTimestamps !== undefined && typeof entityTimestamps !== "boolean") {
    findings.push({ path: "/entityTimestamps", message: "must be a boolean" });
  }

  // filters
  const filtersRaw = raw.filters;
  let filters: { allowed?: string[]; preset?: { q?: string; attrs?: string[]; idPattern?: string; scopeQ?: string } } | undefined;
  if (filtersRaw !== undefined) {
    if (!isObject(filtersRaw)) {
      findings.push({ path: "/filters", message: "must be an object" });
    } else {
      const f: typeof filters = {};
      for (const k of Object.keys(filtersRaw)) {
        if (!["allowed", "preset"].includes(k)) {
          findings.push({ path: `/filters/${k}`, message: "unknown property" });
        }
      }
      if (filtersRaw.allowed !== undefined) {
        if (!Array.isArray(filtersRaw.allowed) || !filtersRaw.allowed.every((a: unknown) => typeof a === "string")) {
          findings.push({ path: "/filters/allowed", message: "must be an array of strings" });
        } else {
          f.allowed = filtersRaw.allowed as string[];
        }
      }
      if (filtersRaw.preset !== undefined) {
        if (!isObject(filtersRaw.preset)) {
          findings.push({ path: "/filters/preset", message: "must be an object" });
        } else {
          const p: typeof f.preset = {};
          for (const k of Object.keys(filtersRaw.preset)) {
            if (!["q", "attrs", "idPattern", "scopeQ"].includes(k)) {
              findings.push({ path: `/filters/preset/${k}`, message: "unknown property" });
            }
          }
          if (filtersRaw.preset.q !== undefined) {
            if (typeof filtersRaw.preset.q !== "string") findings.push({ path: "/filters/preset/q", message: "must be a string" });
            else p.q = filtersRaw.preset.q;
          }
          if (filtersRaw.preset.attrs !== undefined) {
            if (!Array.isArray(filtersRaw.preset.attrs) || !filtersRaw.preset.attrs.every((a: unknown) => typeof a === "string")) {
              findings.push({ path: "/filters/preset/attrs", message: "must be an array of strings" });
            } else p.attrs = filtersRaw.preset.attrs as string[];
          }
          if (filtersRaw.preset.idPattern !== undefined) {
            if (typeof filtersRaw.preset.idPattern !== "string") findings.push({ path: "/filters/preset/idPattern", message: "must be a string" });
            else p.idPattern = filtersRaw.preset.idPattern;
          }
          if (filtersRaw.preset.scopeQ !== undefined) {
            if (typeof filtersRaw.preset.scopeQ !== "string") findings.push({ path: "/filters/preset/scopeQ", message: "must be a string" });
            else p.scopeQ = filtersRaw.preset.scopeQ;
          }
          f.preset = p;
        }
      }
      filters = f;
    }
  }

  const pageSize = raw.pageSize;
  if (pageSize !== undefined) {
    if (typeof pageSize !== "number" || !Number.isInteger(pageSize) || pageSize < 1 || pageSize > MAX_PAGE_SIZE) {
      findings.push({ path: "/pageSize", message: `must be an integer between 1 and ${MAX_PAGE_SIZE}` });
    }
  }

  const mode = raw.mode;
  if (mode !== undefined && !VALID_MODES.has(mode as string)) {
    findings.push({ path: "/mode", message: `must be one of ${[...VALID_MODES].join(", ")}` });
  }

  const editableAttrs = raw.editableAttrs;
  if (editableAttrs !== undefined) {
    if (!Array.isArray(editableAttrs) || !editableAttrs.every((a: unknown) => typeof a === "string")) {
      findings.push({ path: "/editableAttrs", message: "must be an array of strings" });
    } else {
      const computed = new Set((columns ?? []).filter((c) => c.formula !== undefined).map((c) => c.attr));
      editableAttrs.forEach((attr: string, i: number) => {
        if (computed.has(attr)) findings.push({ path: `/editableAttrs/${i}`, message: `"${attr}" is a computed column and cannot be edited` });
      });
    }
  }

  // mode "edit" with empty editableAttrs is a finding
  if (mode === "edit" && (!Array.isArray(editableAttrs) || editableAttrs.length === 0)) {
    findings.push({ path: "/editableAttrs", message: "mode 'edit' requires at least one editable attribute" });
  }

  const historyRaw = raw.history;
  let history: { enabled?: boolean; maxPoints?: number } | undefined;
  if (historyRaw !== undefined) {
    if (!isObject(historyRaw)) {
      findings.push({ path: "/history", message: "must be an object" });
    } else {
      const h: typeof history = {};
      for (const k of Object.keys(historyRaw)) {
        if (!["enabled", "maxPoints"].includes(k)) {
          findings.push({ path: `/history/${k}`, message: "unknown property" });
        }
      }
      if (historyRaw.enabled !== undefined) {
        if (typeof historyRaw.enabled !== "boolean") findings.push({ path: "/history/enabled", message: "must be a boolean" });
        else h.enabled = historyRaw.enabled;
      }
      if (historyRaw.maxPoints !== undefined) {
        if (typeof historyRaw.maxPoints !== "number" || !Number.isInteger(historyRaw.maxPoints) || historyRaw.maxPoints < 1) {
          findings.push({ path: "/history/maxPoints", message: "must be a positive integer" });
        } else h.maxPoints = historyRaw.maxPoints;
      }
      history = h;
    }
  }

  const compareWith = raw.compareWith !== undefined ? checkSource(raw.compareWith, "/compareWith", findings) : undefined;

  const density = raw.density;
  if (density !== undefined && !VALID_DENSITIES.has(density as string)) {
    findings.push({ path: "/density", message: `must be one of ${[...VALID_DENSITIES].join(", ")}` });
  }

  const rowActions = raw.rowActions;
  if (rowActions !== undefined) {
    if (!Array.isArray(rowActions) || !rowActions.every((a: unknown) => typeof a === "string")) {
      findings.push({ path: "/rowActions", message: "must be an array of strings" });
    }
  }

  const mapRaw = raw.map;
  let map: { enabled?: boolean; attr?: string; position?: "right" | "bottom" } | undefined;
  if (mapRaw !== undefined) {
    if (!isObject(mapRaw)) {
      findings.push({ path: "/map", message: "must be an object" });
    } else {
      const m: NonNullable<typeof map> = {};
      for (const k of Object.keys(mapRaw)) {
        if (!["enabled", "attr", "position"].includes(k)) {
          findings.push({ path: `/map/${k}`, message: "unknown property" });
        }
      }
      if (mapRaw.enabled !== undefined) {
        if (typeof mapRaw.enabled !== "boolean") {
          findings.push({ path: "/map/enabled", message: "must be a boolean" });
        } else {
          m.enabled = mapRaw.enabled;
        }
      }
      if (mapRaw.attr !== undefined) {
        if (!isNonEmptyString(mapRaw.attr)) {
          findings.push({ path: "/map/attr", message: "must be a non-empty string" });
        } else {
          m.attr = mapRaw.attr;
        }
      }
      if (mapRaw.position !== undefined) {
        if (mapRaw.position !== "right" && mapRaw.position !== "bottom") {
          findings.push({ path: "/map/position", message: 'must be "right" or "bottom"' });
        } else {
          m.position = mapRaw.position;
        }
      }
      map = m;
    }
  }

  if (findings.length > 0) {
    return { findings };
  }

  const config: ResolvedGridConfig = {
    source: source!,
    type: type as string,
    columns: columns ?? [],
    entityTimestamps: (entityTimestamps as boolean | undefined) ?? false,
    filters: filters ?? {},
    pageSize: (pageSize as number | undefined) ?? DEFAULT_PAGE_SIZE,
    mode: (mode as "view" | "edit") ?? "view",
    editableAttrs: (editableAttrs as string[] | undefined) ?? [],
    history: history ?? { enabled: false },
    compareWith: compareWith,
    density: (density as Density) ?? "comfortable",
    rowActions: (rowActions as string[] | undefined) ?? [],
    // `enabled: false` is the default, so a type with a GeoProperty gets a map only where the
    // manifest asked for one; `position` defaults beside the rows on a wide page.
    map: map ?? { enabled: false },
  };

  return { config, findings };
}
