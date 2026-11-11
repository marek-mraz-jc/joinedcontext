/**
 * The Dashboard manifest as JSON text (T-6791): what the JSON editor starts from, the snippet the
 * widget picker inserts, the kind's schema with the Endpoints the person may read, and where a
 * parse error is. No React here, so each piece is tested on its own.
 */
import type { WidgetSpec } from "../../components/dashboards/DashboardWidgets";

const API_VERSION = "joinedcontext.com/v1alpha1";

/** The widget types the picker inserts: each needs only an Endpoint and an entity type. */
export const PICKED_WIDGETS = ["vega-lite", "grid"] as const;
export type PickedWidget = (typeof PICKED_WIDGETS)[number];

export interface DashboardManifest {
  apiVersion: string;
  kind: "Dashboard";
  metadata: { name: string; namespace?: string };
  spec: {
    title: string | Record<string, string>;
    visibility?: string;
    pages: { title?: string; layout?: string; layers?: string[]; widgets?: WidgetSpec[] }[];
  };
}

/** A new dashboard: one page, no widget yet, visible to the project. */
export function starter(project: string): DashboardManifest {
  return {
    apiVersion: API_VERSION,
    kind: "Dashboard",
    metadata: { name: "", namespace: project },
    spec: { title: "", visibility: "project", pages: [{ title: "Overview", widgets: [] }] },
  };
}

/** The text the editor shows for a manifest. */
export function toText(manifest: unknown): string {
  return `${JSON.stringify(manifest, null, 2)}\n`;
}

/**
 * A valid widget of `type` reading `endpoint`'s `entityType`. A `vega-lite` widget reads the
 * Endpoint's CSV of the type and draws a bar per value of `type` over `{"name": "source"}`, the
 * dataset the platform fills (ADR-N-053 §2.2): never a URL.
 */
export function widgetSnippet(type: PickedWidget, endpoint: string, entityType: string): WidgetSpec {
  if (type === "grid") {
    return { widgetType: "grid", endpointRef: endpoint, entityType };
  }
  return {
    widgetType: "vega-lite",
    endpointRef: endpoint,
    data: { source: "csv", type: entityType },
    spec: {
      $schema: "https://vega.github.io/schema/vega-lite/v5.json",
      title: `${entityType} by type`,
      data: { name: "source" },
      mark: "bar",
      encoding: {
        x: { field: "type", type: "nominal", title: "Type" },
        y: { aggregate: "count", type: "quantitative", title: "Entities" },
      },
    },
  };
}

/** `text` with `widget` added to its last page, or `null` when the text is not a manifest. */
export function insertWidget(text: string, widget: WidgetSpec): string | null {
  const parsed = parseManifest(text);
  if (!parsed.manifest) {
    return null;
  }
  const manifest = parsed.manifest;
  const pages = Array.isArray(manifest.spec.pages) && manifest.spec.pages.length > 0
    ? manifest.spec.pages
    : [{ title: "Overview", widgets: [] }];
  const last = pages[pages.length - 1];
  last.widgets = [...(Array.isArray(last.widgets) ? last.widgets : []), widget];
  manifest.spec.pages = pages;
  return toText(manifest);
}

export interface Parsed {
  manifest?: DashboardManifest;
  /** Why the text is not a manifest, with the line and column the parser stopped at. */
  error?: { message: string; line?: number; column?: number };
}

/** The 1-based line and column of `offset` in `text`. */
function position(text: string, offset: number): { line: number; column: number } {
  const before = text.slice(0, offset).split("\n");
  return { line: before.length, column: (before.at(-1)?.length ?? 0) + 1 };
}

/** The text read as a Dashboard manifest, or why it is not one. */
export function parseManifest(text: string): Parsed {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const offset = /position (\d+)/.exec(message)?.[1];
    return { error: { message, ...(offset ? position(text, Number(offset)) : {}) } };
  }
  const manifest = value as Partial<DashboardManifest> | null;
  if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) {
    return { error: { message: "the text is not a JSON object" } };
  }
  if (manifest.kind !== "Dashboard") {
    return { error: { message: "kind must be \"Dashboard\"" } };
  }
  if (!manifest.spec || typeof manifest.spec !== "object" || !Array.isArray(manifest.spec.pages)) {
    return { error: { message: "spec.pages must be a list of pages" } };
  }
  if (!manifest.metadata || typeof manifest.metadata.name !== "string") {
    return { error: { message: "metadata.name must be the dashboard's name" } };
  }
  return { manifest: manifest as DashboardManifest };
}

type Schema = Record<string, unknown>;

/**
 * The kind's schema (`GET /api/v1/kinds/Dashboard/schema`) with `endpointRef` limited to the
 * Endpoints the person may read: the editor completes from that list and marks any other name.
 * The server checks the reference again when the dashboard is proposed.
 */
export function schemaWithEndpoints(schema: Schema, endpoints: string[]): Schema {
  const copy = structuredClone(schema) as Schema & { definitions?: Record<string, Schema> };
  const widget = copy.definitions?.Widget as { properties?: Record<string, Schema> } | undefined;
  if (widget?.properties?.endpointRef && endpoints.length > 0) {
    widget.properties.endpointRef = {
      ...widget.properties.endpointRef,
      type: "string",
      enum: endpoints,
    };
  }
  return copy;
}
