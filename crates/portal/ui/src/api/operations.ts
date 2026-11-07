import { api, forPeople } from "./client";

/**
 * One call of a project operation (`POST /ops/{name}`, API/01), through the typed client (UI-07).
 *
 * The route is one for every operation, so the document publishes its input and output as JSON
 * values; each operation's own schemas are what `GET /ops` lists. The caller names the output it
 * reads, and gets the status and the reason when the operation refused.
 */
export type OperationAnswer =
  | { ok: true; status: number; output: unknown }
  | { ok: false; status: number; reason?: string };

export async function callOperation(
  project: string,
  name: string,
  input: Record<string, unknown>,
): Promise<OperationAnswer> {
  const { data, error, response } = await api.POST("/api/v1/projects/{project}/ops/{name}", {
    params: { path: { project, name } },
    body: input,
  });
  if (data !== undefined) {
    return { ok: true, status: response.status, output: data };
  }
  const problem = (error ?? {}) as { detail?: unknown; error?: unknown };
  const reason = [problem.detail, problem.error].find(
    (value): value is string => typeof value === "string" && value.length > 0,
  );
  return { ok: false, status: response.status, reason: reason === undefined ? undefined : forPeople(reason) };
}
