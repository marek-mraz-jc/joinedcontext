import { ApiError, forPeople } from "./client";
import { doorAction } from "./odata";

/**
 * One call of a project operation by name (API/01), through the `portal_operation_run` door: its
 * input stays in the action's body, where a function of the registry would carry it in the URL.
 *
 * Each operation's own schemas are what `GET /ops` lists, so the answer is a JSON value: the caller
 * names the output it reads, and gets the status and the reason when the operation refused.
 */
export type OperationAnswer =
  | { ok: true; status: number; output: unknown }
  | { ok: false; status: number; reason?: string };

export async function callOperation(
  project: string,
  name: string,
  input: Record<string, unknown>,
): Promise<OperationAnswer> {
  try {
    return { ok: true, status: 200, output: await doorAction("portal_operation_run", { name, body: input }, project) };
  } catch (error) {
    if (!(error instanceof ApiError)) throw error;
    // An empty sentence is no reason: the caller says its own for the status.
    const reason = error.problem?.detail || error.message || undefined;
    return { ok: false, status: error.status, reason: reason === undefined ? undefined : forPeople(reason) };
  }
}
