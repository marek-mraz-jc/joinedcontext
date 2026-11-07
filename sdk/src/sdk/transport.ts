import type { JcConfig } from "./config";

export type Method = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

export interface JcRequest {
  method: Method;
  path: string;
  body?: unknown;
  /** Bytes sent as they are, with their type, in place of a JSON `body` (the `files` service). */
  raw?: { data: Blob | ArrayBuffer | string; contentType: string };
  /** `"blob"`: a successful answer's body is read as a `Blob`, whatever its type. */
  as?: "blob";
  /**
   * Why a write is made, in the person's words: sent as `JC-Reason`, percent-encoded so any
   * language fits a header, and kept with the write's provenance (DM-113). It also sends
   * `JC-Record: version`, so the replaced value is kept as a version (T-6333).
   */
  reason?: string;
}

export interface JcResponse {
  status: number;
  body: unknown;
  /**
   * The answer's headers, by lowercase name, where the transport has them. A caller that needs one
   * has to survive its absence: a bridged request carries none, and the gateway removes
   * `NGSILD-Results-Count` from an answer it narrowed, because the difference between the count
   * and the rows is the number of entities withheld (R22).
   */
  headers?: Record<string, string>;
}

export type Transport = (request: JcRequest) => Promise<JcResponse>;

export const CSRF_COOKIE = "__Host-jc_csrf";
export const CSRF_HEADER = "x-csrf-token";

function getCookie(doc: Document | undefined, name: string): string {
  if (!doc || !doc.cookie) return "";
  const found = doc.cookie.split("; ").find((c) => c.startsWith(`${name}=`));
  return found ? decodeURIComponent(found.slice(name.length + 1)) : "";
}

export function originTransport(fetchImpl?: typeof fetch, doc?: Document): Transport {
  const doFetch = fetchImpl ?? (typeof fetch !== "undefined" ? fetch.bind(globalThis) : undefined);
  return async (request: JcRequest): Promise<JcResponse> => {
    if (!doFetch) {
      return { status: 0, body: { title: "Fetch is not available in current environment." } };
    }
    const headers: Record<string, string> = {
      Accept: "application/json",
    };

    if (request.method !== "GET") {
      const token = getCookie(doc ?? (typeof document !== "undefined" ? document : undefined), CSRF_COOKIE);
      if (token) {
        headers[CSRF_HEADER] = token;
      }
      if (request.raw) {
        headers["content-type"] = request.raw.contentType;
      } else if (request.body !== undefined) {
        headers["content-type"] = "application/json";
      }
      if (request.reason !== undefined) {
        headers["JC-Reason"] = encodeURIComponent(request.reason);
        // A write a person names a reason for is an edit by hand: the broker keeps the value it
        // replaces as a version, shown in the row's Changes (ADR-N-054 2.9, T-6333).
        headers["JC-Record"] = "version";
      }
    }

    try {
      const response = await doFetch(request.path, {
        method: request.method,
        credentials: "same-origin",
        headers,
        body: request.raw
          ? request.raw.data
          : request.body !== undefined && request.method !== "GET"
            ? JSON.stringify(request.body)
            : undefined,
      });

      const answered: Record<string, string> = {};
      response.headers.forEach((value, name) => {
        answered[name.toLowerCase()] = value;
      });

      if (response.status === 204) {
        return { status: 204, body: null, headers: answered };
      }

      const contentType = response.headers.get("content-type") ?? "";
      let body: unknown = null;
      if (request.as === "blob" && response.ok) {
        body = await response.blob();
      } else if (contentType.includes("json")) {
        body = await response.json().catch(() => null);
      } else {
        const text = await response.text().catch(() => "");
        body = text === "" ? null : text;
      }
      return { status: response.status, body, headers: answered };
    } catch (err) {
      return {
        status: 0,
        body: { title: err instanceof Error ? err.message : String(err) },
      };
    }
  };
}

/**
 * What the preview's observer reads (SDK-27): the bridge requests still waiting for the Portal,
 * when one last started or settled, and the ones the Portal answered with an error status.
 */
export interface RequestActivity {
  pending: number;
  lastChange: number;
  failed: { path: string; status: number }[];
}

/** At most this many failed requests are kept, each path cut to this length (API/04 §5). */
export const MAX_FAILED_REQUESTS = 50;
export const MAX_FAILED_PATH = 256;

export const activity: RequestActivity = { pending: 0, lastChange: Date.now(), failed: [] };

/** Forgets every request: a test starts from a quiet document. */
export function resetActivity(): void {
  activity.pending = 0;
  activity.lastChange = Date.now();
  activity.failed = [];
}

export interface BridgeOptions {
  target?: Window;
  self?: Window;
  timeoutMs?: number;
}

export function bridgeTransport(options?: BridgeOptions): Transport {
  let seq = 0;
  return (request: JcRequest): Promise<JcResponse> => {
    const target = options?.target ?? (typeof window !== "undefined" ? window.parent : undefined);
    const self = options?.self ?? (typeof window !== "undefined" ? window : undefined);
    const timeoutMs = options?.timeoutMs ?? 15000;
    const id = ++seq;

    activity.pending += 1;
    activity.lastChange = Date.now();

    return new Promise((resolve) => {
      let timer: ReturnType<typeof setTimeout> | undefined;

      const settle = (response: JcResponse) => {
        activity.pending = Math.max(0, activity.pending - 1);
        activity.lastChange = Date.now();
        if (response.status >= 400 && response.status <= 599 && activity.failed.length < MAX_FAILED_REQUESTS) {
          activity.failed.push({ path: request.path.slice(0, MAX_FAILED_PATH), status: response.status });
        }
        resolve(response);
      };

      const cleanup = () => {
        if (timer) clearTimeout(timer);
        self?.removeEventListener("message", onMessage);
      };

      const onMessage = (event: MessageEvent) => {
        if (event.source !== target) return;
        const data = event.data as Partial<{ kind: string; id: number; status: number; body: unknown }> | null;
        if (!data || data.kind !== "jc-response" || data.id !== id || typeof data.status !== "number") return;
        cleanup();
        settle({ status: data.status, body: data.body });
      };

      self?.addEventListener("message", onMessage);

      timer = setTimeout(() => {
        cleanup();
        settle({ status: 0, body: { title: "The Portal did not answer." } });
      }, timeoutMs);

      target?.postMessage(
        {
          kind: "jc-request",
          id,
          method: request.method,
          path: request.path,
          body: request.body,
          reason: request.reason,
        },
        "*",
      );
    });
  };
}

export function transportFor(config: JcConfig): Transport {
  if (config.transport !== "origin") {
    return bridgeTransport();
  }
  // A published app is the whole of its own host (AP-133): its endpoint calls go to that host's
  // `/api/endpoint/`, where the edge sets the app's session as the bearer (T-2670).
  return originTransport();
}
