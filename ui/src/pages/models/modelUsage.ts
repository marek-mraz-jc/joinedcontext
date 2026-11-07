/**
 * Who uses a DataModel, computed from the project's manifests (DM-61, DM-62, T-2765, T-2766).
 *
 * A space has one model and names it (`dataModelRef`), or the model names its space
 * (`contextSpaceRef`); everything that reads or writes that space's data uses the model. The
 * Endpoints serve the space, the Subscriptions watch it, the Apps declare a data need on it, the
 * Pipelines read through one of its Endpoints or read or produce one of its types, the Dashboards
 * draw one of its Endpoints (in a widget, or in a Layer a page names), and a Mapping names the
 * model as its source or its target. Nothing here
 * asks the API: it is read off the lists the caller already holds, which are the lists the
 * person may read, so a use the person may not see is never counted.
 */
import { refName } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { entityTypesOf, spaceOf } from "../spaces/spaceFacts";

export type UsingKind = "ContextSpace" | "Endpoint" | "Pipeline" | "Subscription" | "App" | "Dashboard" | "Mapping";

/** The kinds in the order a person reads them, with the plural their pages live under. */
export const USING_KINDS: readonly { kind: UsingKind; plural: string }[] = [
  { kind: "ContextSpace", plural: "spaces" },
  { kind: "Endpoint", plural: "endpoints" },
  { kind: "Pipeline", plural: "pipelines" },
  { kind: "Subscription", plural: "subscriptions" },
  { kind: "App", plural: "apps" },
  { kind: "Dashboard", plural: "dashboards" },
  { kind: "Mapping", plural: "mappings" },
];

/**
 * The project's manifests by plural (`spaces`, `endpoints`, `pipelines`, `subscriptions`, `apps`,
 * `dashboards`, `layers`, `mappings`); a list the caller did not load is simply absent.
 */
export type ProjectManifests = Partial<Record<string, Manifest[]>>;

export interface ModelUse {
  kind: UsingKind;
  plural: string;
  name: string;
}

/** The space a model belongs to: the one it names, else the one that names it (DM-61). */
export function spaceOfModel(model: Manifest, spaces: Manifest[] = []): string | undefined {
  return (
    spaceOf(model) ??
    spaces.find((space) => refName(space.spec.dataModelRef) === model.metadata.name)?.metadata.name
  );
}

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

function records(value: unknown): Record<string, unknown>[] {
  return Array.isArray(value) ? value.map(record) : [];
}

/** Every Endpoint a Dashboard draws: its widgets', and the Layers its pages name, on every page. */
function dashboardEndpoints(dashboard: Manifest, layers: Manifest[]): string[] {
  const layerEndpoint = new Map(layers.map((layer) => [layer.metadata.name, refName(layer.spec.sourceEndpointRef)]));
  return records(dashboard.spec.pages).flatMap((page) => [
    ...records(page.widgets).map((widget) => refName(widget.endpointRef)),
    ...(Array.isArray(page.layers) ? page.layers : []).map((layer) =>
      typeof layer === "string" ? (layerEndpoint.get(layer) ?? "") : "",
    ),
  ]);
}

/** Whether a Pipeline reads through one of the Endpoints or reads or writes one of the types. */
function pipelineUses(pipeline: Manifest, endpoints: Set<string>, types: Set<string>): boolean {
  const source = record(pipeline.spec.source);
  const read = refName(source.endpointRef);
  const queried = record(source.query).type;
  const triggered = record(record(source.trigger).subscription).type;
  const written = record(pipeline.spec.output).type;
  return (
    (read !== "" && endpoints.has(read)) ||
    [queried, triggered, written].some((type) => typeof type === "string" && types.has(type))
  );
}

/**
 * Everything of the project that uses `model`, kind by kind in `USING_KINDS` order, each list
 * sorted by name. The model's own space comes first, as the thing the others hang off.
 */
export function usesOfModel(model: Manifest, all: ProjectManifests): ModelUse[] {
  const name = model.metadata.name;
  const space = spaceOfModel(model, all.spaces);
  const types = new Set(entityTypesOf(model));
  const inSpace = (manifest: Manifest) => space !== undefined && spaceOf(manifest) === space;
  const endpoints = (all.endpoints ?? []).filter(inSpace).map((endpoint) => endpoint.metadata.name);
  const served = new Set(endpoints);

  const found: Record<UsingKind, string[]> = {
    ContextSpace: space !== undefined && (all.spaces ?? []).some((one) => one.metadata.name === space) ? [space] : [],
    Endpoint: endpoints,
    Pipeline: (all.pipelines ?? []).filter((one) => pipelineUses(one, served, types)).map((one) => one.metadata.name),
    Subscription: (all.subscriptions ?? []).filter(inSpace).map((one) => one.metadata.name),
    App: (all.apps ?? [])
      .filter((app) => records(app.spec.dataNeeds).some((need) => space !== undefined && refName(need.contextSpaceRef) === space))
      .map((one) => one.metadata.name),
    Dashboard: (all.dashboards ?? [])
      .filter((dashboard) => dashboardEndpoints(dashboard, all.layers ?? []).some((endpoint) => served.has(endpoint)))
      .map((one) => one.metadata.name),
    Mapping: (all.mappings ?? [])
      .filter((mapping) => [mapping.spec.source, mapping.spec.target].some((end) => refName(end) === name))
      .map((one) => one.metadata.name),
  };
  return USING_KINDS.flatMap(({ kind, plural }) =>
    [...new Set(found[kind])].sort().map((used) => ({ kind, plural, name: used })),
  );
}

/** How many of each kind use the model, for a list's "used by" column. */
export function usageCounts(uses: ModelUse[]): Partial<Record<UsingKind, number>> {
  const counts: Partial<Record<UsingKind, number>> = {};
  for (const use of uses) {
    counts[use.kind] = (counts[use.kind] ?? 0) + 1;
  }
  return counts;
}
