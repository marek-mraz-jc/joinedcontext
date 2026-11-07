import { api, unwrap } from "./client";
import type { components } from "./schema";

/**
 * What a space's data model serves of the surfaces that need one (EP-125, T-5299): each `sta` or
 * `ical` with the sentence the Portal refuses it with, or none when the model serves it.
 */
export type SurfaceFit = components["schemas"]["SurfaceFit"];

export function surfaceFitQuery(project: string, space: string) {
  return {
    queryKey: ["surface-fit", project, space] as const,
    enabled: space.length > 0,
    retry: false,
    queryFn: async (): Promise<SurfaceFit> =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/spaces/{space}/surface-fit", {
          params: { path: { project, space } },
        }),
      ),
  };
}

/** The surfaces the model cannot serve, each with its sentence. */
export function surfaceGaps(fit: SurfaceFit | undefined): { representation: string; gap: string }[] {
  return (fit?.surfaces ?? []).flatMap(({ representation, gap }) =>
    typeof gap === "string" && gap.length > 0 ? [{ representation, gap }] : [],
  );
}

/**
 * The checkboxes to grey out: the always-on ones and each surface with a gap, unless the Endpoint
 * already lists it, so a person can still uncheck what the model cannot serve.
 */
export function disabledRepresentations(
  locked: readonly string[],
  fit: SurfaceFit | undefined,
  listed: readonly string[] | undefined,
): string[] {
  const gapped = surfaceGaps(fit)
    .map(({ representation }) => representation)
    .filter((representation) => !(listed ?? []).includes(representation));
  return [...new Set([...locked, ...gapped])];
}
