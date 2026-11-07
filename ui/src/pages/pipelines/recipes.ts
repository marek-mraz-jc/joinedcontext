/**
 * Pipeline recipes (T-3258): the pipelines a city starts from, each asking only for its source and
 * where it writes, and filled into the pipeline form the workbench then samples, maps and tests
 * before anything is proposed. The mapping of a load recipe is left to the workbench, which drafts
 * it from the first record the source answers (T-3211); the KPI recipe writes its aggregation.
 */
import type { Manifest } from "../../api/manifest";
import type { PipelineForm } from "./PipelineEditor";
import { findKpiTargetEndpoint, kpiBloblang } from "./PipelineStudio";
import { spaceOf } from "../spaces/spaceFacts";

export type RecipeId = "csv" | "api" | "ckan" | "kpi";

export interface Recipe {
  id: RecipeId;
  /** Where the source comes from: a DataSource of the project, or an Endpoint for the KPI. */
  source: "datasource" | "endpoint";
}

export const RECIPES: Recipe[] = [
  { id: "csv", source: "datasource" },
  { id: "api", source: "datasource" },
  { id: "ckan", source: "datasource" },
  { id: "kpi", source: "endpoint" },
];

/** What a person gave a recipe. `target` is the Endpoint URN a load writes through. */
export interface RecipeValues {
  source: string;
  target?: string;
  /** The KPI's entity type and the numeric attribute it averages. */
  type?: string;
  attribute?: string;
}

/** Why a recipe cannot be filled yet, as a key under `pipelines.recipe.problem`. */
export type RecipeProblem = "source" | "target" | "type" | "attribute";

export function problemOf(recipe: Recipe, values: RecipeValues): RecipeProblem | undefined {
  if (!values.source.trim()) return "source";
  if (recipe.id === "kpi") {
    if (!values.type?.trim()) return "type";
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(values.attribute?.trim() ?? "")) return "attribute";
    return undefined;
  }
  return values.target?.trim() ? undefined : "target";
}

/** A DNS-1123 name for the pipeline from its source and target, at most 63 characters. */
export function nameOf(...parts: string[]): string {
  const label = parts
    .join("-")
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 63)
    .replace(/-+$/, "");
  return label || "pipeline";
}

/**
 * The form a recipe fills. A file published once a day is read daily at 03:00 UTC (a CKAN
 * resource at 04:00), an API is polled every 15 minutes, and a KPI is computed every 15 minutes.
 */
export function fromRecipe(
  recipe: Recipe,
  values: RecipeValues,
  context: { project: string; endpoints: Manifest[]; orgDomain?: string },
): PipelineForm {
  const source = values.source.trim();
  if (recipe.id === "kpi") {
    const type = values.type?.trim() ?? "";
    const attribute = values.attribute?.trim() ?? "";
    const name = nameOf(source, attribute, "avg");
    const endpoint = context.endpoints.find((one) => one.metadata.name === source);
    return {
      name,
      class: "auto",
      period: "15m",
      source: { endpointRef: source, query: { type, attrs: [attribute] } },
      compute: {
        kind: "bloblang",
        bloblang: kpiBloblang({
          kpiName: name,
          project: context.project,
          sourceEndpoint: source,
          sourceSpace: (endpoint ? spaceOf(endpoint) : undefined) ?? context.project,
          type,
          attribute,
          aggregate: "average",
          period: "15m",
        }),
      },
      output: { type: "KeyPerformanceIndicator", mode: "upsert" },
      targetEndpoint: findKpiTargetEndpoint(context.project, context.endpoints, context.orgDomain),
    };
  }
  const target = values.target?.trim() ?? "";
  const timing: Partial<PipelineForm> =
    recipe.id === "api"
      ? { class: "auto", period: "15m" }
      : { class: "scheduled", schedule: recipe.id === "ckan" ? "0 4 * * *" : "0 3 * * *" };
  return {
    name: nameOf(source, "to", target.split(":").pop() ?? ""),
    ...timing,
    source: { dataSourceRef: source },
    output: { mode: "upsert" },
    targetEndpoint: target,
  } as PipelineForm;
}
