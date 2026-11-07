import { api, unwrap } from "./client";
import type { components } from "./schema";

/**
 * Calls per Endpoint, surface and day (T-5118): what the gateways counted, names and numbers only.
 * The rows grow every five minutes, so a page keeps an answer for one.
 */
export type EndpointUsage = components["schemas"]["EndpointUsage"];
export type ProjectUsage = components["schemas"]["ProjectUsage"];

/** The windows the Usage section offers; the server takes 1 to 90 days. */
export const WINDOWS = [7, 30, 90] as const;
export type UsageWindow = (typeof WINDOWS)[number];

/** The surfaces in the order the gateway names them (src/endpoint_usage.rs SURFACES). */
export const SURFACES = ["ngsi-ld", "ogc", "odata", "sta", "ical", "feed", "mcp", "websocket", "download"] as const;

/** The kinds of caller a count names (src/endpoint_usage.rs CALLERS, T-6186); never who called. */
export const CALLERS = ["person", "workload", "agent", "anonymous", "unknown"] as const;

const FIVE_MINUTES = 5 * 60 * 1000;

/** An answer that is no usage (a proxy's page, an older Portal) is said as such, never drawn. */
function checked<T>(answer: unknown, list: "days" | "endpoints"): T {
  if (typeof answer !== "object" || answer === null || !Array.isArray((answer as Record<string, unknown>)[list])) {
    throw new Error("the Portal answered no usage");
  }
  return answer as T;
}

export function endpointUsageQuery(project: string, name: string, days: UsageWindow) {
  return {
    queryKey: ["endpoint-usage", project, name, days] as const,
    staleTime: FIVE_MINUTES,
    queryFn: async ({ signal }: { signal: AbortSignal }): Promise<EndpointUsage> =>
      checked<EndpointUsage>(
        await unwrap(
          await api.GET("/api/v1/projects/{project}/endpoints/{name}/usage", {
            params: { path: { project, name }, query: { days } },
            signal,
          }),
        ),
        "days",
      ),
  };
}

export function projectUsageQuery(project: string, days: UsageWindow) {
  return {
    queryKey: ["project-usage", project, days] as const,
    staleTime: FIVE_MINUTES,
    queryFn: async ({ signal }: { signal: AbortSignal }): Promise<ProjectUsage> =>
      checked<ProjectUsage>(
        await unwrap(
          await api.GET("/api/v1/projects/{project}/usage/endpoints", {
            params: { path: { project }, query: { days } },
            signal,
          }),
        ),
        "endpoints",
      ),
  };
}

/** One month (`YYYY-MM`) of the project's usage as the CSV the Portal wrote. */
export async function exportUsage(project: string, month: string): Promise<string> {
  const csv: unknown = await unwrap(
    await api.GET("/api/v1/projects/{project}/usage/endpoints/export", {
      params: { path: { project }, query: { month } },
      parseAs: "text",
    }),
  );
  return typeof csv === "string" ? csv : "";
}

/** This month in UTC as `YYYY-MM`, the month the rows are kept by. */
export function thisMonth(now: Date = new Date()): string {
  return now.toISOString().slice(0, 7);
}
