import { api, ApiError, unwrap } from "./client";
import type { components } from "./schema";

/**
 * How much a space holds (T-2889, API/01 §29): the broker's entity count of the space, which the
 * Portal keeps for five minutes, so the page keeps it as long and never retries a refusal.
 */
export type SpaceUsage = components["schemas"]["SpaceUsage"];

const FIVE_MINUTES = 5 * 60 * 1000;

/**
 * A count the broker has not given in this time is not coming: the page says so (T-4225). It
 * outwaits the Portal's own 20 s wait for the broker (src/space_usage.rs TIMEOUT), so the page
 * never drops an answer the Portal is about to give: at 15 s every Helsinki count on dev showed
 * "—" while the Portal still waited (T-4825).
 */
export const USAGE_TIMEOUT_MS = 25_000;

export function spaceUsageQuery(project: string, space: string) {
  return {
    queryKey: ["space-usage", project, space] as const,
    staleTime: FIVE_MINUTES,
    retry: false,
    queryFn: async ({ signal }: { signal: AbortSignal }): Promise<SpaceUsage> => {
      const call = new AbortController();
      const timer = setTimeout(
        () => call.abort(new DOMException("the count did not come", "TimeoutError")),
        USAGE_TIMEOUT_MS,
      );
      signal.addEventListener("abort", () => call.abort(signal.reason));
      try {
        return unwrap(
          await api.GET("/api/v1/projects/{project}/spaces/{space}/usage", {
            params: { path: { project, space } },
            signal: call.signal,
          }),
        );
      } finally {
        clearTimeout(timer);
      }
    },
  };
}

/** Why a count cannot be shown, in the server's words when it gave some. */
export function usageRefusal(error: unknown): string {
  if (error instanceof ApiError) {
    return error.problem?.detail ?? error.message;
  }
  return error instanceof Error ? error.message : "";
}

const BINARY = ["B", "KiB", "MiB", "GiB", "TiB"] as const;

/** A size in binary units with the locale's separators: 1,5 MiB, 340 KiB, 12 B (T-2890). */
export function sizeText(bytes: number, language: string): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < BINARY.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit > 0 && value < 10 ? 1 : 0;
  return `${new Intl.NumberFormat(language, { maximumFractionDigits: digits }).format(value)} ${BINARY[unit]}`;
}

/**
 * What one entity weighs as the gateway answered it: the UTF-8 length of its JSON (T-6145). The
 * broker's own row size (`byType`) holds every attribute expanded, with its metadata, plus the
 * row's own columns, so it runs several times a keyValues sample (T-6156, `spaces.inside.sizeHint`).
 */
export function entityBytes(entity: unknown): number {
  return new TextEncoder().encode(JSON.stringify(entity) ?? "").length;
}

/** The exact bytes, for the tooltip: 1 572 864 B. */
export function exactBytes(bytes: number, language: string): string {
  return `${new Intl.NumberFormat(language).format(bytes)} B`;
}

/**
 * The bytes of one type of the space's DataModel. The broker keys them by the stored type, an
 * expanded IRI, and the model names the type short, so a key matches by itself or by its last
 * segment; a short name two IRIs share is no answer.
 */
export function bytesOfType(byType: Record<string, number> | undefined, type: string): number | undefined {
  if (!byType) return undefined;
  if (byType[type] !== undefined) return byType[type];
  const matches = Object.entries(byType).filter(([key]) => key.split(/[/#]/).pop() === type);
  return matches.length === 1 ? matches[0][1] : undefined;
}

/**
 * Asks the broker to measure the space again (T-6208). Concurrent asks share one broker read and
 * an answer under 30 s old stands, so a second click costs the broker nothing.
 */
export async function recalculateSpaceUsage(project: string, space: string): Promise<SpaceUsage> {
  return unwrap(
    await api.POST("/api/v1/projects/{project}/spaces/{space}/usage/recalculate", {
      params: { path: { project, space } },
    }),
  );
}
