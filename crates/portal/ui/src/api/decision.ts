/**
 * Approving or rejecting one change (CC-34, PF-58), wherever a person decides it: its own page or
 * the inbox (T-3273). The 202 carries the change with its new phase, which updates the change in
 * place; the project's list is read again.
 */
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "./client";
import type { components } from "./schema";
import { approvalStanding } from "./approval";
import { usePermissions } from "./permissions";
import { useAuth } from "../auth/AuthProvider";

type ChangeProposal = components["schemas"]["ChangeProposal"];

export function useDecision(project: string, id: string) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);

  const settle = (change: { status: ChangeProposal["status"] }) => {
    queryClient.setQueryData(queryKeys.change(project, id), (prev?: ChangeProposal) =>
      prev ? { ...prev, status: change.status } : prev,
    );
    // `exact`, or the prefix match would also refetch this change and undo the line above.
    void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project), exact: true });
  };
  const failed = (err: unknown) => {
    if (err instanceof ApiError) setError(err.problem?.detail ?? err.message);
    else if (err instanceof Error) setError(err.message);
    else setError(t("app.error.generic"));
  };

  const approve = useMutation({
    mutationFn: async (confirm: string | undefined) => {
      setError(null);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/changes/{id}/approve", {
          params: { path: { project, id } },
          body: confirm !== undefined ? { confirm: confirm.trim() } : undefined,
        }),
      );
    },
    onSuccess: settle,
    onError: failed,
  });

  const reject = useMutation({
    mutationFn: async (reason: string) => {
      setError(null);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/changes/{id}/reject", {
          params: { path: { project, id } },
          // The reason goes into the merge request's closing comment, where the proposer reads it.
          body: { reason: reason.trim() },
        }),
      );
    },
    onSuccess: settle,
    onError: failed,
  });

  return { approve, reject, error, pending: approve.isPending || reject.isPending };
}

/** The project's change list, read once for every hook below (one query key). */
function useChangeList(project: string) {
  return useQuery({
    queryKey: queryKeys.changes(project),
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/changes", { params: { path: { project } } })),
    refetchInterval: 30_000,
  });
}

/**
 * Why the project's changes did not load, or null: an inbox that read a failed list as empty
 * said "Nothing waits for your decision" while changes waited (T-4966).
 */
export function useDecisionsError(project: string): string | null {
  const list = useChangeList(project);
  if (!list.isError || list.data) return null;
  return list.error instanceof ApiError ? (list.error.problem?.detail ?? list.error.message) : list.error.message;
}

/** The changes of `project` waiting for this person's decision: pending and approvable by them. */
export function useDecisions(project: string): ChangeProposal[] {
  const { identity } = useAuth();
  const permissions = usePermissions(project);
  const list = useChangeList(project);
  return (list.data?.items ?? []).filter(
    (change) =>
      change.status.phase === "PendingApproval" &&
      approvalStanding(permissions, identity?.email ?? undefined, change).block === null,
  );
}
