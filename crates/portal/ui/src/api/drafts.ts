import { ApiError } from "./client";
import { act, call, projectBound } from "./odata";
import { activeWorkspace } from "../components/layout/WorkspaceContext";
import type { components } from "./schema";
export { canonicalizeJson, digestOf } from "./digest";

// UI-07: the draft shapes are the Portal API's own, from the generated schema, never a copy.
export type Verdict = components["schemas"]["Verdict"];
export type Draft = components["schemas"]["Draft"];
export type DraftEvent = components["schemas"]["DraftEvent"];

/** The copy the person works in, as the REST doors read it from `?workspace=` (CC-76). */
function inWorkspace(): { workspace?: string } {
  const workspace = activeWorkspace();
  return workspace ? { workspace } : {};
}

/** The problem's own sentence of a refusal, which names a refused field and never its value (MF-24). */
function detailOf(error: unknown): string | undefined {
  const detail = error instanceof ApiError ? error.problem?.detail : undefined;
  return typeof detail === "string" ? detail : undefined;
}

function statusOf(error: unknown): number | undefined {
  return error instanceof ApiError ? error.status : undefined;
}

/**
 * One draft, or null when there is none (404, or an answer that is not a draft), read with the
 * registry's `jc_draft_get` through the OData service (T-7119) in the workspace the person is in
 * (CC-76). Any other failure throws with its status and detail: read as "no draft", it would let
 * the form write with no expected version over another window's draft (F-UI1-013).
 */
export async function getDraft(
  project: string,
  kind: string,
  name: string,
): Promise<Draft | null> {
  let answer: unknown;
  try {
    answer = await call("jc_draft_get", project, { kind, name, ...inWorkspace() });
  } catch (error) {
    if (statusOf(error) === 404) return null;
    const detail = detailOf(error);
    throw Object.assign(new Error(detail ?? (error instanceof Error ? error.message : String(error))), {
      status: statusOf(error),
      detail,
    });
  }
  try {
    return asDraft(answer);
  } catch {
    return null;
  }
}

/** A draft answer carries a version; anything else is not a draft (a mocked or proxied 200). */
function asDraft(body: unknown): Draft {
  if (!body || typeof body !== "object" || typeof (body as Draft).version !== "number") {
    throw new Error("not a draft");
  }
  return body as Draft;
}

/** Writes the shared draft with `jc_draft_put`; another window's newer version throws 409. */
export async function putDraft(
  project: string,
  kind: string,
  name: string,
  manifest: unknown,
  expectedVersion?: number,
): Promise<Draft> {
  let answer: unknown;
  try {
    answer = await act(
      "jc_draft_put",
      projectBound(project),
      {
        kind,
        name,
        manifest,
        ...(typeof expectedVersion === "number" ? { expectedVersion } : {}),
        ...inWorkspace(),
      },
    );
  } catch (error) {
    const status = statusOf(error);
    if (status === 409) {
      throw Object.assign(new Error("draft conflict"), { status: 409 });
    }
    // `detail` is the server's own sentence; the dialog shows it where the person is typing.
    const detail = detailOf(error);
    throw Object.assign(
      new Error(`failed to put draft (${status ?? "no answer"})${detail ? `: ${detail}` : ""}`),
      { status, detail },
    );
  }
  return asDraft(answer);
}

/**
 * Throws away one shared draft (AG-61) with `jc_draft_drop`. The server checks the person may
 * write drafts of the project; a refusal throws with its detail.
 */
export async function dropDraft(project: string, kind: string, name: string): Promise<void> {
  await act("jc_draft_drop", projectBound(project), { kind, name, ...inWorkspace() });
}

const DRAFT_EVENTS = ["message", "draft", "put", "verdict", "drop"] as const;

export function subscribeDrafts(
  project: string,
  onEvent: (event: DraftEvent) => void,
): () => void {
  const Source = (globalThis as { EventSource?: typeof EventSource }).EventSource;
  if (!Source) {
    return () => {};
  }
  const source = new Source(
    `/api/v1/projects/${encodeURIComponent(project)}/drafts/events`,
    { withCredentials: true },
  );

  const handleMessage = (msg: MessageEvent<string>) => {
    try {
      onEvent(JSON.parse(msg.data) as DraftEvent);
    } catch {
      // ignore non-json messages
    }
  };

  for (const event of DRAFT_EVENTS) {
    source.addEventListener(event, handleMessage as EventListener);
  }

  return () => {
    for (const event of DRAFT_EVENTS) {
      source.removeEventListener(event, handleMessage as EventListener);
    }
    source.close();
  };
}
