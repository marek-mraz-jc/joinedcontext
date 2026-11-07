import init, { analyse } from "../wasm/pkg/news_topics_wasm.js";

let initPromise: Promise<unknown> | null = null;

function ensureInit(): Promise<unknown> {
  if (!initPromise) {
    initPromise = init();
  }
  return initPromise;
}

export interface WorkerRequest {
  id: number;
  input: unknown;
}

export interface WorkerSuccessResponse {
  id: number;
  output: unknown;
}

export interface WorkerErrorResponse {
  id: number;
  error: string;
}

export type WorkerResponse = WorkerSuccessResponse | WorkerErrorResponse;

self.onmessage = async (event: MessageEvent<WorkerRequest>) => {
  const data = event.data;
  if (!data || typeof data.id !== "number") return;
  const { id, input } = data;

  try {
    await ensureInit();
    const raw = typeof input === "string" ? input : JSON.stringify(input);
    const json = analyse(raw);
    const parsed = JSON.parse(json) as Record<string, unknown>;
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      "error" in parsed &&
      typeof parsed.error === "string"
    ) {
      self.postMessage({ id, error: parsed.error } satisfies WorkerErrorResponse);
    } else {
      self.postMessage({ id, output: parsed } satisfies WorkerSuccessResponse);
    }
  } catch (err) {
    const error = err instanceof Error ? err.message : String(err);
    self.postMessage({ id, error } satisfies WorkerErrorResponse);
  }
};
