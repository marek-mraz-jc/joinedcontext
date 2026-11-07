import { api } from "./client";
import type { components } from "./schema";
export { canonicalizeJson, digestOf } from "./digest";

// UI-07: the draft shapes are the Portal API's own, from the generated schema, never a copy.
export type Verdict = components["schemas"]["Verdict"];
export type Draft = components["schemas"]["Draft"];
export type DraftEvent = components["schemas"]["DraftEvent"];

/**
 * One draft, or null when there is none (404, or an answer that is not a draft). Through the typed
 * client, so the workspace the person is in reaches the call (CC-76) and an ended session goes to
 * the login. Any other failure throws with its status and detail: read as "no draft", it would let
 * the form write with no expected version over another window's draft (F-UI1-013).
 */
export async function getDraft(
  project: string,
  kind: string,
  name: string,
): Promise<Draft | null> {
  const { data, error, response } = await api.GET("/api/v1/projects/{project}/drafts/{kind}/{name}", {
    params: { path: { project, kind, name } },
  });
  if (data !== undefined) {
    try {
      return asDraft(data);
    } catch {
      return null;
    }
  }
  if (response.status === 404) return null;
  const detail = (error as { detail?: unknown } | undefined)?.detail;
  throw Object.assign(new Error(typeof detail === "string" ? detail : `HTTP ${response.status}`), {
    status: response.status,
    detail: typeof detail === "string" ? detail : undefined,
  });
}

/** A draft answer carries a version; anything else is not a draft (a mocked or proxied 200). */
function asDraft(body: unknown): Draft {
  if (!body || typeof body !== "object" || typeof (body as Draft).version !== "number") {
    throw new Error("not a draft");
  }
  return body as Draft;
}

export async function putDraft(
  project: string,
  kind: string,
  name: string,
  manifest: unknown,
  expectedVersion?: number,
): Promise<Draft> {
  const body: components["schemas"]["PutDraftRequest"] = { manifest };
  if (typeof expectedVersion === "number") {
    body.expectedVersion = expectedVersion;
  }
  const { data, error, response } = await api.PUT(
    "/api/v1/projects/{project}/drafts/{kind}/{name}",
    { params: { path: { project, kind, name } }, body },
  );
  if (response.status === 409) {
    const current = (error as { current?: unknown } | undefined)?.current;
    throw Object.assign(new Error("draft conflict"), {
      status: 409,
      current: typeof current === "number" ? current : undefined,
    });
  }
  if (data === undefined) {
    const detail = (error as { detail?: unknown } | undefined)?.detail;
    // `detail` is the server's own sentence, which names a refused field and never its value
    // (MF-24); the dialog shows it where the person is typing.
    throw Object.assign(
      new Error(`failed to put draft (${response.status})${typeof detail === "string" ? `: ${detail}` : ""}`),
      { status: response.status, detail: typeof detail === "string" ? detail : undefined },
    );
  }
  return asDraft(data);
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
