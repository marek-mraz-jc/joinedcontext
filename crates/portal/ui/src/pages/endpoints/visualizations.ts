import type { Manifest } from "../../api/manifest";

/** The chart templates jc-core fills (T-4642, EP-130), each with the roles it places a column in. */
export const TEMPLATES = {
  bar: { required: ["x", "y"], optional: ["color"] },
  line: { required: ["x", "y"], optional: ["color"] },
  heatmap: { required: ["x", "y", "color"], optional: [] },
  "small-multiples": { required: ["x", "y", "facet"], optional: [] },
  "linked-brush": { required: ["x", "y"], optional: ["color"] },
} as const;

export type TemplateName = keyof typeof TEMPLATES;
export type Role = "x" | "y" | "color" | "facet";
type Texts = Record<string, string>;

/** `spec.catalog.visualizations[]` of an Endpoint, as jc-core's `Visualization` reads it. */
export interface Visualization {
  name: string;
  title: Texts;
  description: Texts;
  data: Record<string, unknown> & { source: string };
  spec: Record<string, unknown>;
}

/** The source a template reads first, by the representations the Endpoint serves. */
const PREFERRED: Record<TemplateName, string[]> = {
  bar: ["odata", "csv"],
  heatmap: ["odata", "csv"],
  line: ["temporal", "csv"],
  "small-multiples": ["temporal", "csv"],
  "linked-brush": ["temporal", "csv"],
};

/** The representation each source reads, as jc-core's `VisualizationData::representation`. */
const REPRESENTATION: Record<string, string> = { csv: "csv", odata: "odata", temporal: "ngsi-ld", ogc: "ogc-features" };

/**
 * The data block a new chart starts from: a time series reads the last day of its measure hour by
 * hour, a comparison the entity set; `undefined` when the Endpoint serves none of the sources.
 */
export function defaultData(
  template: TemplateName,
  type: string,
  measure: string,
  served: string[],
): Visualization["data"] | undefined {
  const source = PREFERRED[template].find((one) => served.includes(REPRESENTATION[one] ?? one));
  switch (source) {
    case "temporal":
      return { source, type, attrs: [measure], last: "PT24H", aggrMethod: "avg", aggrPeriod: "PT1H" };
    case "odata":
      return { source, entitySet: type };
    case "csv":
      return { source, type };
    default:
      return undefined;
  }
}

/** Whether every role the template needs has a column. */
export function complete(template: TemplateName, columns: Partial<Record<Role, string>>): boolean {
  return TEMPLATES[template].required.every((role) => Boolean(columns[role]));
}

/** The Endpoint's visualizations, `[]` when it has none. */
export function visualizationsOf(manifest: Manifest): Visualization[] {
  const catalog = (manifest.spec as { catalog?: { visualizations?: Visualization[] } }).catalog;
  return catalog?.visualizations ?? [];
}

/** The Endpoint manifest with this list; the rest of its catalogue block stays as it was. */
export function withVisualizations(manifest: Manifest, list: Visualization[]): Manifest {
  const spec = manifest.spec as { catalog?: Record<string, unknown> };
  const catalog = { ...spec.catalog };
  if (list.length > 0) {
    catalog.visualizations = list;
  } else {
    delete catalog.visualizations;
  }
  const { status: _status, ...rest } = manifest as Manifest & { status?: unknown };
  void _status;
  const next = { ...spec, catalog };
  if (Object.keys(catalog).length === 0) {
    delete (next as { catalog?: unknown }).catalog;
  }
  return { ...rest, spec: next } as Manifest;
}
