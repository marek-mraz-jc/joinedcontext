import { ApiError, type ProblemDetails } from "./client";
import { isChange, ORG_NAMESPACE } from "./manifest";
import { door, doorAction } from "./odata";
import type { Change } from "./manifest";
import type { components } from "./schema";

/**
 * A model's LinkML source, read and proposed through the Portal's model source doors (UI-07,
 * DM-56, T-7177): each runs the route's handler, its query and its two answers, on the
 * Organization for its own models (the `org` namespace) and in the project for a project's.
 */

export type SourceDryRun = components["schemas"]["SourceDryRunResult"];

/**
 * The committed source of one model; throws the ApiError of a refusal, so a page tells a 403
 * from a lost connection (T-6950).
 */
export async function readModelSource(project: string, name: string): Promise<string> {
  const source =
    project === ORG_NAMESPACE
      ? await door("portal_org_model_source", { name })
      : await door("portal_model_source", { name }, project);
  // The route answers text/yaml, which the door hands back as a string; anything else is no source.
  if (typeof source !== "string") throw new ApiError(502, `The source of ${name} came back as no text.`);
  return source;
}

/** What one source write came back with: the check's verdict, the Change, or why it was refused. */
export type SourceAnswer =
  | { kind: "checked"; result: SourceDryRun }
  | { kind: "proposed"; change: Change }
  | { kind: "refused"; status: number; problem: Partial<ProblemDetails> };

/**
 * Checks (`dryRun`) or proposes a model's source. `space` travels with a create only: for a model
 * that exists the route reads the space off its manifest. The door sends the YAML as it was
 * typed, as the route reads `text/yaml`, never a JSON document.
 */
export async function writeModelSource(options: {
  project: string;
  name: string;
  source: string;
  dryRun: boolean;
  space?: string;
}): Promise<SourceAnswer> {
  const { project, name, source, space } = options;
  const params = { name, source, dryRun: options.dryRun ? "All" : undefined, space };
  let answer: unknown;
  try {
    answer =
      project === ORG_NAMESPACE
        ? await doorAction("portal_org_model_source_set", params)
        : await doorAction("portal_model_source_set", params, project);
  } catch (error) {
    if (!(error instanceof ApiError)) throw error;
    return { kind: "refused", status: error.status, problem: error.problem ?? { detail: error.message } };
  }
  if (options.dryRun && answer !== null && typeof answer === "object") {
    return { kind: "checked", result: answer as SourceDryRun };
  }
  // A proposal answers its Change; a check's verdict or nothing is no proposal.
  if (!options.dryRun && isChange(answer)) return { kind: "proposed", change: answer };
  const detail = options.dryRun ? `The check of ${name} came back with no verdict.` : `${name} came back with no Change.`;
  return { kind: "refused", status: 502, problem: { detail } };
}
