import type { Mock } from "vitest";

/** One call a fetch mock received, read the same way whether it was a Request or a URL and init. */
export interface Sent {
  /** Path and query, without the origin. */
  path: string;
  method: string;
  /** The body as text; empty for a call without one. */
  text: () => Promise<string>;
  json: () => Promise<unknown>;
}

export function sent(call: unknown[]): Sent {
  const [input, init] = call as [RequestInfo | URL, RequestInit | undefined];
  const raw = input instanceof Request ? input.url : input instanceof URL ? input.href : String(input);
  const url = new URL(raw, window.location.origin);
  const text = async (): Promise<string> => {
    if (init?.body != null) return String(init.body);
    return input instanceof Request ? input.clone().text() : "";
  };
  return {
    path: `${url.pathname}${url.search}`,
    method: init?.method ?? (input instanceof Request ? input.method : "GET"),
    text,
    json: async () => JSON.parse(await text()) as unknown,
  };
}

/** Every call of a fetch mock whose path contains `fragment`, oldest first. */
export function sentTo(mock: Mock, fragment: string): Sent[] {
  return mock.mock.calls.map(sent).filter((call) => call.path.includes(fragment));
}
