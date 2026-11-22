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

/** The ranges a reader's toolbar opens on (T-7491), as the source's `last`. */
export const RANGES = { day: "PT24H", week: "P7D", month: "P31D" } as const;
/** The granularities, as the source's `aggrPeriod`; `raw` reads every value. */
export const GRANULARITIES = { raw: undefined, hourly: "PT1H", daily: "P1D" } as const;
/** The aggregation methods the editor offers. */
export const METHODS = ["avg", "min", "max"] as const;

/** What the reader's toolbar starts from, and whether readers get it at all. */
export interface TemporalDefaults {
  range: keyof typeof RANGES;
  granularity: keyof typeof GRANULARITIES;
  method: (typeof METHODS)[number];
  controls: boolean;
}

/** The last day hour by hour, averaged, with the toolbar shown. */
export const DEFAULT_TEMPORAL: TemporalDefaults = { range: "day", granularity: "hourly", method: "avg", controls: true };

/** The source a template reads, by the representations the Endpoint serves; `undefined` for none. */
export function sourceOf(template: TemplateName, served: string[]): string | undefined {
  return PREFERRED[template].find((one) => served.includes(REPRESENTATION[one] ?? one));
}

/**
 * The data block a new chart starts from: a time series reads its measure over the range at the
 * granularity the editor picked, a comparison the entity set; `undefined` when the Endpoint serves
 * none of the sources.
 */
export function defaultData(
  template: TemplateName,
  type: string,
  measure: string,
  served: string[],
  temporal: TemporalDefaults = DEFAULT_TEMPORAL,
): Visualization["data"] | undefined {
  const source = sourceOf(template, served);
  switch (source) {
    case "temporal": {
      const period = GRANULARITIES[temporal.granularity];
      return {
        source,
        type,
        attrs: [measure],
        last: RANGES[temporal.range],
        ...(period ? { aggrMethod: temporal.method, aggrPeriod: period } : {}),
        ...(temporal.controls ? {} : { controls: false }),
      };
    }
    case "odata":
      return { source, entitySet: type };
    case "csv":
      return { source, type };
    default:
      return undefined;
  }
}

/** The toolbar setting each member of a temporal source is, as the editor names its field. */
const SETTINGS = { last: "range", aggrPeriod: "granularity", aggrMethod: "method", controls: "controls" } as const;

/** One toolbar setting a refusal names: the chart's place in the list that was sent, and the field. */
export interface RefusedSetting {
  index: number;
  field: (typeof SETTINGS)[keyof typeof SETTINGS];
}

/**
 * The toolbar settings a refusal names, when that is all it names (T-7491): the page then says it
 * in its own words. `[]` for any other refusal, whose sentence is shown as the server wrote it.
 */
export function refusedSettings(lines: readonly string[]): RefusedSetting[] {
  const out: RefusedSetting[] = [];
  for (const line of lines) {
    const at = /^(?:invalid )?\/spec\/catalog\/visualizations\/(\d+)\/data\/(last|aggrPeriod|aggrMethod|controls)\b/.exec(line);
    if (!at) return [];
    out.push({ index: Number(at[1]), field: SETTINGS[at[2] as keyof typeof SETTINGS] });
  }
  return out;
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
