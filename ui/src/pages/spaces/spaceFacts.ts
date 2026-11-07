/**
 * What the Portal reads about a space from its manifests (T-3280 moved these out of the space's
 * page, so a module that only asks which space a manifest belongs to does not load the page).
 */
import type { Manifest } from "../../api/manifest";
import { refName } from "../../api/manifest";
import { admitsPerson } from "../../components/endpoints/sharing";
import { policyRefsOf } from "../../components/endpoints/policyBinding";
import { SPACE_LABEL } from "../../schemas/modelprojection";

const RESULTS_COUNT_HEADER = "NGSILD-Results-Count";

/** The space a manifest belongs to: `spec.contextSpaceRef` first, the space label second. */
/**
 * The DataModels of a space: the one its optional `dataModelRef` names first, then every model
 * whose required `contextSpaceRef` names the space (T-2450). The list and the space's own page
 * both read it, so the list no longer says "—" where the page says "helsinki" (T-2760).
 */
export function modelsOfSpace(space: Manifest, models: Manifest[]): Manifest[] {
  const pointer = refName(space.spec.dataModelRef);
  const primary = models.find((m) => m.metadata.name === pointer);
  const owned = models.filter((m) => m !== primary && spaceOf(m) === space.metadata.name);
  return [...(primary ? [primary] : []), ...owned];
}

export function spaceOf(manifest: Manifest): string | undefined {
  return (
    (refName(manifest.spec.contextSpaceRef) || undefined) ??
    (manifest.metadata.labels as Record<string, string> | null | undefined)?.[SPACE_LABEL]
  );
}

/** An App's own Endpoint, written by the Portal beside the App (T-2759). */
export function isAppEndpoint(endpoint: Manifest): boolean {
  return endpoint.metadata.annotations?.["joinedcontext.com/generated-by"] === "portal/app-reconciler";
}

/**
 * The endpoint the portal reads a space through: one without a policy narrows nothing, so it
 * shows everything the space holds; failing that the first public one is at least readable.
 * Given the person's `groups`, only an endpoint whose audience admits them is picked, and `null`
 * (the identity not known yet) picks none: a page does not fetch what the gateway will refuse
 * (T-2631). The space's own endpoints come before an App's: an App's Endpoint serves its App's
 * roles, so picking it gave two people two different reads of one type (T-5604).
 */
export function pickReadEndpoint(
  endpoints: Manifest[],
  groups?: readonly string[] | null,
  project = "",
): Manifest | undefined {
  if (groups === null) {
    return undefined;
  }
  const readable = groups === undefined ? endpoints : endpoints.filter((endpoint) => admitsPerson(endpoint, groups, project));
  const pick = (among: Manifest[]) =>
    among.find((endpoint) => policyRefsOf(endpoint) === undefined) ??
    among.find((endpoint) => endpoint.spec.audience === "public");
  return pick(readable.filter((endpoint) => !isAppEndpoint(endpoint))) ?? pick(readable.filter(isAppEndpoint));
}

/**
 * The entity types a DataModel defines: `spec.classes` when the manifest lists them, else the
 * class names of an inline LinkML source (`spec.linkml` carrying a document, not a path).
 */
export function entityTypesOf(model: Manifest): string[] {
  const classes = model.spec.classes;
  if (Array.isArray(classes)) {
    return classes.filter((c): c is string => typeof c === "string");
  }
  return [];
}

/** The `NGSILD-Results-Count` header as a number; `undefined` when absent or not a number. */
export function parseResultsCount(headers: Headers): number | undefined {
  const raw = headers.get(RESULTS_COUNT_HEADER);
  if (raw === null) {
    return undefined;
  }
  const count = Number.parseInt(raw.trim(), 10);
  return Number.isNaN(count) || count < 0 ? undefined : count;
}

