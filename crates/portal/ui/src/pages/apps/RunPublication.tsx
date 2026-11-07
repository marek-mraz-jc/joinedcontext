import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { approvalStanding, changedKind } from "../../api/approval";
import { usePermissions } from "../../api/permissions";
import { useIdentity } from "../../auth/AuthProvider";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { Button, buttonClass, InlineError, SourceLink } from "../../components/ui";

/**
 * Where an application lives and how it goes live (AP-71): its source in Git and its copy on GitHub
 * where there is one (AP-79), and after Publish
 * the Change that publishes it, approved right here by a person the approval rules allow
 * (CC-34, PF-50, PF-58). A red-lane Change is approved on its own page, where its name is typed
 * back (CC-39).
 */
export function RunPublication({
  project,
  sourceUrl,
  mirrorUrl,
  changeId,
  onApproved,
}: {
  project: string;
  sourceUrl?: string;
  /** The application's copy on GitHub, where the installation keeps one (AP-79). */
  mirrorUrl?: string;
  changeId?: string;
  onApproved?: () => void;
}): JSX.Element | null {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const identity = useIdentity();
  const permissions = usePermissions(project);

  const change = useQuery({
    queryKey: queryKeys.change(project, changeId ?? ""),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/changes/{id}", {
          params: { path: { project, id: changeId ?? "" } },
        }),
      ),
    enabled: Boolean(changeId),
    refetchInterval: (query) =>
      query.state.data?.status.phase === "Deploying" ? 5_000 : false,
  });

  const approve = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/changes/{id}/approve", {
          params: { path: { project, id: changeId ?? "" } },
        }),
      ),
    onSuccess: (approved) => {
      queryClient.setQueryData(queryKeys.change(project, changeId ?? ""), (prev: unknown) =>
        prev && typeof prev === "object" ? { ...prev, status: approved.status } : prev,
      );
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project), exact: true });
      onApproved?.();
    },
  });

  if (!sourceUrl && !changeId) {
    return null;
  }

  const proposal = change.data;
  const pending = proposal?.status.phase === "PendingApproval";
  const red = proposal?.status.lane === "red";
  const standing = proposal
    ? approvalStanding(permissions, identity?.email ?? undefined, proposal)
    : null;

  return (
    <section
      aria-label={t("agentRun.publication.title")}
      className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2 rounded border border-border p-3 text-body"
    >
      {sourceUrl ? (
        <SourceLink href={sourceUrl} label={t("agentRun.publication.source")} />
      ) : null}
      {mirrorUrl ? (
        <SourceLink href={mirrorUrl} label={t("agentRun.publication.mirror")} />
      ) : null}

      {changeId ? (
        <span className="flex min-w-0 flex-wrap items-center gap-2">
          <span className="font-medium">{t("agentRun.publication.title")}</span>
          <span className="font-mono text-caption text-fg-muted">{changeId}</span>
          {proposal ? (
            <>
              <LifecycleBadge kind="lane" value={proposal.status.lane} />
              <LifecycleBadge kind="phase" value={proposal.status.phase} />
            </>
          ) : null}
        </span>
      ) : null}

      {changeId && pending && standing ? (
        <span className="flex min-w-0 flex-wrap items-center gap-2">
          {standing.block === null && !red ? (
            <Button
              variant="primary"
              size="sm"
              disabled={approve.isPending}
              disabledReason={t("app.busyReason")}
              onClick={() => approve.mutate()}
            >
              {approve.isPending ? t("approvals.approving") : t("agentRun.publication.approve")}
            </Button>
          ) : null}
          <Link
            to="/projects/$project/approvals/$id"
            params={{ project, id: changeId }}
            className={buttonClass("secondary", "sm")}
          >
            {t("changes.review")}
          </Link>
          {standing.ownAsAdministrator ? (
            <span className="text-caption text-fg-muted">
              {t("approvals.ownAsAdministrator", { kind: proposal ? changedKind(proposal) : "App" })}
            </span>
          ) : null}
          {standing.block ? (
            <span className="text-caption text-fg-muted">{t(`approvals.${standing.block}`)}</span>
          ) : null}
        </span>
      ) : null}

      {approve.error ? (
        <InlineError className="basis-full">
          {approve.error instanceof ApiError
            ? (approve.error.problem?.detail ?? approve.error.message)
            : t("app.error.generic")}
        </InlineError>
      ) : null}
    </section>
  );
}
