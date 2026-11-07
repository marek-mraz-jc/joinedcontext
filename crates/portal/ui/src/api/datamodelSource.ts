import { api } from "./client";
import { asProblem, type ProblemDetails } from "./client";
import type { Change } from "./manifest";
import type { components } from "./schema";

/**
 * A model's LinkML source, read and proposed through the typed client (UI-07, DM-56): the route,
 * its query and its two answers are the API's own, and the session and csrf handling are the
 * client's, not a copy of them.
 */

export type SourceDryRun = components["schemas"]["SourceDryRunResult"];

const SOURCE = "/api/v1/projects/{project}/datamodels/{name}/source" as const;

/** The YAML is sent as it was typed: the route reads `text/yaml`, never a JSON string. */
const asYaml = { bodySerializer: (body: string) => body, headers: { "Content-Type": "text/yaml; charset=utf-8" } };

/** The committed source of one model; throws with the status when it cannot be read. */
export async function readModelSource(project: string, name: string): Promise<string> {
  const { data, response } = await api.GET(SOURCE, {
    params: { path: { project, name } },
    headers: { Accept: "text/yaml, text/plain, */*" },
    parseAs: "text",
  });
  if (data === undefined) {
    throw new Error(`HTTP ${response.status}`);
  }
  return data;
}

/** What one source write came back with: the check's verdict, the Change, or why it was refused. */
export type SourceAnswer =
  | { kind: "checked"; result: SourceDryRun }
  | { kind: "proposed"; change: Change }
  | { kind: "refused"; status: number; problem: Partial<ProblemDetails> };

/**
 * Checks (`dryRun`) or proposes a model's source. `space` travels with a create only: for a model
 * that exists the route reads the space off its manifest.
 */
export async function writeModelSource(options: {
  project: string;
  name: string;
  source: string;
  dryRun: boolean;
  space?: string;
}): Promise<SourceAnswer> {
  const query: { dryRun?: string; space?: string } = {};
  if (options.dryRun) query.dryRun = "All";
  if (options.space) query.space = options.space;
  const { data, error, response } = await api.PUT(SOURCE, {
    params: { path: { project: options.project, name: options.name }, query },
    body: options.source,
    ...asYaml,
  });
  if (options.dryRun && response.status === 200 && data) {
    return { kind: "checked", result: data as SourceDryRun };
  }
  if (!options.dryRun && response.status === 202 && data) {
    return { kind: "proposed", change: data as Change };
  }
  const problem: Partial<ProblemDetails> = asProblem(error);
  return { kind: "refused", status: response.status, problem };
}
