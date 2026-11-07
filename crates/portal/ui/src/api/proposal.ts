import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "./client";
import { isChange } from "./manifest";
import type { Change, ResourceProposal } from "./manifest";
import type { components } from "./schema";

/** What a dry run answers, as far as the check before a proposal reads it. */
type CheckAnswer = {
  valid?: boolean;
  verdict?: { ok?: boolean; findings?: { message?: string }[] };
};

/**
 * Checks a manifest, then proposes it (PF-57, T-0956). Every proposal needs a fresh green check,
 * whatever door it comes through, so a page that has no Check button of its own runs the dry run
 * on the same route and verb, with the same body, and proposes only when it is green; a red check
 * stops here with what it found, and nothing is proposed.
 */
export async function proposeChecked(
  project: string,
  plural: string,
  body: ResourceProposal,
  create: boolean,
  /** The ETag the manifest was read with: an update the server holds a newer one of is a 412. */
  ifMatch?: string,
): Promise<unknown> {
  const send = (dryRun: boolean) => {
    const query = dryRun ? { dryRun: "All" } : undefined;
    return create
      ? api.POST("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural }, query },
          body,
        })
      : api.PUT("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural, name: body.metadata.name }, query, header: ifMatch ? { "If-Match": ifMatch } : undefined },
          body,
        });
  };
  const check = (await unwrap(await send(true))) as CheckAnswer;
  if (check.valid === false || check.verdict?.ok === false) {
    const found = (check.verdict?.findings ?? [])
      .map((finding) => finding.message)
      .filter((message): message is string => Boolean(message));
    throw new ApiError(422, found.join("; ") || "the check found problems", {
      type: "about:blank",
      title: "The check found problems",
      status: 422,
      detail: found.join("; ") || "the check found problems",
    });
  }
  return unwrap(await send(false));
}

/**
 * One call of the import door (MF-23, PF-57): `dryRun` checks without proposing. Every caller
 * of `/import` comes through here, so the route, the dry run and the refusal are said once.
 */
export async function importBody(
  project: string,
  body: components["schemas"]["ImportOptions"],
  dryRun: boolean,
): Promise<unknown> {
  return unwrap<unknown>(
    await api.POST("/api/v1/projects/{project}/import", {
      params: { path: { project }, query: dryRun ? { dryRun: "All" } : undefined },
      body,
    }),
  );
}

/**
 * The import checked first and then proposed, as one Change (PF-57): a refused check stops it
 * before anything is proposed.
 */
export async function importChecked(project: string, body: components["schemas"]["ImportOptions"]): Promise<unknown> {
  await importBody(project, body, true);
  return importBody(project, body, false);
}

/**
 * One import of manifests the form drew; `dryRun` checks without proposing. Every manifest still
 * lands as a Change. `replace` (the default) updates what an earlier proposal of the same form
 * already drew (MF-23, T-1227); `fail` refuses a name someone else holds.
 */
export async function importManifests(
  project: string,
  manifests: unknown[],
  dryRun: boolean,
  conflictPolicy: "replace" | "fail" = "replace",
): Promise<unknown> {
  return importBody(project, { manifests: manifests as ResourceProposal[], conflictPolicy }, dryRun);
}

/**
 * Proposes new manifests as one checked import, the way the endpoint form proposes its first
 * projection (PF-57). A name taken since the form loaded, by a concurrent proposal or another
 * person, is refused with the import's detail instead of replaced (F-UI2-013).
 */
export async function proposeBundle(project: string, manifests: unknown[]): Promise<unknown> {
  return importChecked(project, { manifests: manifests as ResourceProposal[], conflictPolicy: "fail" });
}

/** One PUT or POST of a manifest, as the endpoints page does it; the result is a Change. */
export function useProposal(project: string, plural: string, onChange: (change: Change) => void) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const mutation = useMutation({
    mutationFn: async ({
      body,
      create,
      bundle = [],
    }: {
      body: ResourceProposal;
      create: boolean;
      /** Drafted manifests the resource needs, proposed with it as one Change. */
      bundle?: ResourceProposal[];
    }) => {
      setError(null);
      if (bundle.length > 0) {
        // The dry run is the bundle's check, and the import needs it first (PF-57, T-1460).
        // An update proposed with its bundle replaces what the project has, as a lone update does;
        // a create keeps the import's refusal of anything that already exists (MF-23).
        return importChecked(project, {
          manifests: [...bundle, body],
          ...(create ? {} : { conflictPolicy: "replace" as const }),
        });
      }
      return proposeChecked(project, plural, body, create);
    },
    onSuccess: (result, { bundle = [] }) => {
      if (isChange(result)) {
        onChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, plural) });
      if (bundle.length > 0) {
        void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "layers") });
      }
    },
    onError: (err) => {
      setError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      );
    },
  });
  return { mutation, error, reset: () => setError(null) };
}
