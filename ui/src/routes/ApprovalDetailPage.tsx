import { useId, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useDecision } from "../api/decision";
import { Link } from "@tanstack/react-router";
import { approvalStanding, changedKind, expectedNameOf, isOwn, rejectWords } from "../api/approval";
import { usePermissions } from "../api/permissions";
import { api, queryKeys, unwrap } from "../api/client";
import { useAuth } from "../auth/AuthProvider";
import { ConfirmName } from "../components/ChangeNotice";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { PlanDiffViewer } from "../components/diff/PlanDiffViewer";
import { ProposedViaBadge, ProposedViaHint } from "../components/ProposedViaBadge";
import { PublicationPreview } from "../pages/catalogue/PublicationPreview";
import { applyFields } from "../pages/catalogue/publish";
import { localized } from "../api/manifest";
import type { components } from "../api/schema";
import {
  Alert,
  Button,
  Dialog,
  ExternalLink,
  Field,
  PageHeader,
  Skeleton,
  Textarea,
  safeHref,
} from "../components/ui";
import { ResourcePageFailed } from "../components/ui/PageState";

type ChangeProposal = components["schemas"]["ChangeProposal"];
type ChangeFile = components["schemas"]["ChangeFile"];

/** The list page of the changed resource: the folder of its file, `projects/{project}/{plural}/…`. */
function pluralOf(change: ChangeProposal): string | undefined {
  const kind = changedKind(change);
  const file = (change.files ?? []).find((f) => f.kind === kind) ?? change.files?.[0];
  const parts = file?.path.split("/") ?? [];
  return parts[0] === "projects" && parts.length > 3 ? parts[2] : undefined;
}

/** The order the files are read in: what the strictest lane decides comes first. */
const LANE_ORDER: Record<ChangeFile["lane"], number> = { red: 2, yellow: 1, green: 0 };

/** The three words the diff already uses for what a change does to a file. */
const OPERATION_LABEL: Record<ChangeFile["operation"], string> = {
  Create: "approvals.diffAdded",
  Update: "approvals.diffChanged",
  Delete: "approvals.diffRemoved",
};



/** The combined states the forge reports for a commit (CC-90). */
const CI_STATES: readonly string[] = ["success", "pending", "failure", "error"];
export function ApprovalDetailPage({
  project,
  id,
}: {
  project: string;
  id: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const { identity } = useAuth();
  const permissions = usePermissions(project);

  const rejectReasonId = useId();
  const [rejecting, setRejecting] = useState(false);
  const [rejectReason, setRejectReason] = useState("");
  const [confirmInput, setConfirmInput] = useState("");
  // The file whose field diff is shown; none picked shows the headline file (T-1397).
  const [selectedPath, setSelectedPath] = useState<string | null>(null);

  const detailQuery = useQuery({
    queryKey: queryKeys.change(project, id),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/changes/{id}", {
          params: { path: { project, id } },
        }),
      ),
    // Deploying is the one phase the reconciler moves on without the user acting.
    refetchInterval: (query) =>
      query.state.data?.status.phase === "Deploying" ? 5_000 : false,
  });

  const proposal = detailQuery.data;

  // What the approval did, said once the phase moves while the page is open (T-1386): the page
  // polls through Deploying, so the move to Applied is seen here too.
  const [banner, setBanner] = useState<"deploying" | "live" | null>(null);
  const phase = proposal?.status.phase;
  const [seenPhase, setSeenPhase] = useState(phase);
  if (phase !== seenPhase) {
    setSeenPhase(phase);
    if (seenPhase !== undefined && phase === "Deploying") setBanner("deploying");
    else if (seenPhase !== undefined && phase === "Applied") setBanner("live");
  }

  const isRedLane = proposal?.status.lane === "red";
  const expectedName = proposal ? expectedNameOf(proposal) : "";
  const confirmMatches = !isRedLane || confirmInput.trim() === expectedName;

  // Approve on the change's kind, and on someone else's change unless the caller administers
  // that kind (PF-50, PF-58); the API checks the same.
  const standing = proposal
    ? approvalStanding(permissions, identity?.email ?? undefined, proposal)
    : { block: null, ownAsAdministrator: false };
  const disabledReason = standing.block ? t(`approvals.${standing.block}`) : null;

  const decision = useDecision(project, id);
  const approveMutation = decision.approve;
  const rejectMutation = decision.reject;
  const actionError = decision.error;

  if (detailQuery.isPending) {
    return <ChangeSkeleton label={t("app.loading")} />;
  }

  if (detailQuery.isError) {
    return (
      <ResourcePageFailed
        title={id}
        description={t("approvals.lead")}
        error={detailQuery.error}
        onRetry={() => {
          void detailQuery.refetch();
        }}
        back={
          <Link
            to="/projects/$project/approvals"
            params={{ project }}
            className="focus-ring text-body text-primary-soft-fg underline hover:no-underline"
          >
            {t("approvals.back")}
          </Link>
        }
      />
    );
  }

  // `proposal` was read before the guards above, so TypeScript still has it as optional here.
  // A settled, non-error query always carries data; this is the check that says so.
  if (!proposal) {
    return <ChangeSkeleton label={t("app.loading")} />;
  }

  const isMutating = approveMutation.isPending || rejectMutation.isPending;
  const isPendingApproval = proposal.status.phase === "PendingApproval";
  const canAct = !disabledReason && isPendingApproval && !isMutating;
  const canApprove = canAct && confirmMatches;
  // Why Approve is disabled, on the button itself and not only beside it (UI-44).
  const approveReason =
    disabledReason ?? (canAct && !confirmMatches ? t("approvals.confirmFirst", { name: expectedName }) : null);
  // The proposer withdraws their own change, whoever approves it: closing never merges (T-6535).
  const own = isOwn(identity?.email, proposal);
  const words = rejectWords(own);
  const canReject = own ? isPendingApproval && !isMutating : canAct;

  const summaryText = t(
    proposal.summary.key,
    proposal.summary.params as Record<string, unknown>,
  );

  // Red first: the file that decides the confirmation is the one an approver has to read,
  // and a bundle's headline is rarely it (T-0861).
  const files = [...(proposal.files ?? [])].sort(
    (one, other) => LANE_ORDER[other.lane] - LANE_ORDER[one.lane],
  );
  const selected = files.find((file) => file.path === selectedPath && file.fields);

  return (
    <div className="space-y-6">
      <PageHeader
        title={summaryText}
        description={<span className="font-mono">{proposal.metadata.name}</span>}
        aside={
          <div className="flex flex-wrap items-center gap-2">
            <LifecycleBadge kind="lane" value={proposal.status.lane} />
            <LifecycleBadge kind="phase" value={proposal.status.phase} />
            {/* An AI client proposed it for the author (AG-137): the approver reads that first. */}
            {proposal.status.proposedVia ? (
              <ProposedViaBadge via={proposal.status.proposedVia} />
            ) : null}
          </div>
        }
      />

      {/* What this change names comes from another one: it merges after it, and a rejected one
          means what it names will never exist (MF-48). */}
      {(proposal.waitsOn ?? []).map((awaited) => (
        <Alert
          key={awaited.name}
          role="status"
          tone={awaited.phase === "Rejected" ? "danger" : awaited.phase === "Merged" ? "success" : "warning"}
        >
          {awaited.phase === "Rejected"
            ? t("approvals.waitsOn.rejected", { change: awaited.name })
            : awaited.phase === "Merged"
              ? t("approvals.waitsOn.merged", { change: awaited.name })
              : t("approvals.waitsOn.pending", { change: awaited.name })}
        </Alert>
      ))}

      <dl className="grid grid-cols-1 gap-4 rounded border border-border bg-surface-subtle p-4 sm:grid-cols-2">
        <div>
          <dt className="text-xs font-medium text-surface-fg/70">{t("approvals.author")}</dt>
          <dd className="mt-1 text-sm font-medium text-surface-fg">
            {proposal.author.name}
            {proposal.author.email ? (
              <span className="ml-1 text-xs text-fg-muted">({proposal.author.email})</span>
            ) : null}
          </dd>
          {proposal.status.proposedVia ? (
            <ProposedViaHint via={proposal.status.proposedVia} author={proposal.author.name} />
          ) : null}
        </div>
        {proposal.workspace ? (
          <div className="sm:col-span-2" data-testid="change-workspace">
            <dt className="text-xs font-medium text-surface-fg/70">{t("approvals.workspace")}</dt>
            <dd className="mt-1 text-sm text-surface-fg">
              {t("approvals.fromWorkspace", {
                name: proposal.workspace,
                author: proposal.author.name,
                date: new Date(proposal.createdAt).toLocaleDateString(locale),
              })}
            </dd>
          </div>
        ) : null}
        {proposal.status.repository ? (
          <div>
            <dt className="text-xs font-medium text-surface-fg/70">
              {t("approvals.repository")}
            </dt>
            <dd className="mt-1 text-sm text-surface-fg">{proposal.status.repository}</dd>
          </div>
        ) : null}
        {/* The project repository's CI on the head commit (CC-90), in words. */}
        {proposal.ci ? (
          <div data-testid="change-ci">
            <dt className="text-xs font-medium text-surface-fg/70">{t("approvals.ci.label")}</dt>
            <dd className="mt-1 text-sm font-medium text-surface-fg">
              {CI_STATES.includes(proposal.ci)
                ? t(`approvals.ci.${proposal.ci}`)
                : proposal.ci}
            </dd>
          </div>
        ) : null}
        {proposal.status.mergeRequest ? (
          <div>
            <dt className="text-xs font-medium text-surface-fg/70">
              {t("approvals.mergeRequest")}
            </dt>
            <dd className="mt-1 text-sm font-medium">
              <ExternalLink
                href={proposal.status.mergeRequest}
                className="text-primary-soft-fg [overflow-wrap:anywhere]"
              >
                {safeHref(proposal.status.mergeRequest) ? t("approvals.openMergeRequest") : proposal.status.mergeRequest}
              </ExternalLink>
            </dd>
          </div>
        ) : null}
      </dl>

      {files.length > 0 ? (
        <section aria-labelledby="files-heading" className="space-y-2">
          <h2 id="files-heading" className="text-base font-semibold text-surface-fg">
            {t("approvals.files")} ({t("approvals.fileCount", { count: files.length })})
          </h2>
          <p className="text-caption text-fg-muted">{t("approvals.filesLead")}</p>
          <ul className="divide-y divide-border rounded border border-border">
            {files.map((file) => (
              <li
                key={file.path}
                className="flex flex-wrap items-center gap-2 px-3 py-2"
                data-testid="change-file"
              >
                <LifecycleBadge kind="lane" value={file.lane} />
                {file.kind ? (
                  <span className="text-sm font-medium text-surface-fg">{file.kind}</span>
                ) : null}
                <div className="min-w-0 flex-1">
                  {file.fields ? (
                    <Button
                      variant="ghost"
                      size="sm"
                      aria-pressed={selected?.path === file.path}
                      onClick={() => setSelectedPath(selected?.path === file.path ? null : file.path)}
                      className="h-auto min-h-8 px-1 text-left font-mono text-caption text-primary-soft-fg [overflow-wrap:anywhere] hover:underline"
                    >
                      {file.path}
                    </Button>
                  ) : (
                    <span className="font-mono text-caption text-fg-subtle [overflow-wrap:anywhere]">
                      {file.path}
                    </span>
                  )}
                </div>
                <span className="ml-auto text-caption text-fg-muted">
                  {t(OPERATION_LABEL[file.operation])}
                </span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      <section aria-labelledby="plan-diff-heading" className="space-y-2">
        <h2 id="plan-diff-heading" className="text-base font-semibold text-surface-fg">
          {selected ? t("approvals.diffForFile", { path: selected.path }) : t("approvals.diffType")}
        </h2>
        <PlanDiffViewer fields={selected ? selected.fields : proposal.planFields} />
      </section>

      {files
        .filter((file) => file.kind === "Endpoint" && file.operation !== "Delete" && publishes(file))
        .map((file) => (
          <PublicationOnApproval key={file.path} project={project} file={file} />
        ))}

      {banner && proposal ? (
        <Alert role="status" tone={banner === "live" ? "success" : "info"}>
          <span>
            {t(banner === "live" ? "approvals.approvedLive" : "approvals.approvedDeploying", {
              kind: changedKind(proposal),
            })}
          </span>
          {banner === "live" && pluralOf(proposal) ? (
            <Link
              to="/projects/$project/$plural"
              params={{ project, plural: pluralOf(proposal) as string }}
              className="ml-2 font-medium text-primary-soft-fg hover:underline focus:outline-none focus:ring-2 focus:ring-border-focus"
            >
              {t("approvals.openResource", { kind: changedKind(proposal) })}
            </Link>
          ) : null}
        </Alert>
      ) : null}

      {own && rejectMutation.isSuccess ? (
        <Alert role="status" tone="info">
          {t("approvals.withdrawn")}
        </Alert>
      ) : null}

      {actionError ? (
        <Alert role="alert" tone="danger">
          {actionError}
        </Alert>
      ) : null}

      {isPendingApproval ? (
        <section aria-labelledby="actions-heading" className="space-y-4 rounded border border-border p-4">
          <h2 id="actions-heading" className="sr-only">
            {t("approvals.actions")}
          </h2>

          {isRedLane && isPendingApproval ? (
            <ConfirmName
              expected={expectedName}
              value={confirmInput}
              onChange={setConfirmInput}
              disabled={!canAct}
              title={disabledReason ?? (isMutating ? t("app.busyReason") : undefined)}
            />
          ) : null}

          {standing.ownAsAdministrator && isPendingApproval ? (
            <p role="note" className="text-sm text-fg-muted">
              {t("approvals.ownAsAdministrator", { kind: changedKind(proposal) })}
            </p>
          ) : null}

          <div className="flex flex-wrap items-center gap-3">
            <Button
              variant="primary"
              disabled={!canApprove}
              disabledReason={approveReason ?? undefined}
              loading={approveMutation.isPending}
              onClick={() => approveMutation.mutate(isRedLane ? confirmInput : undefined)}
            >
              {approveMutation.isPending ? t("approvals.approving") : t("approvals.approve")}
            </Button>

            <Button
              variant="danger"
              disabled={!canReject}
              // The same sentence the Approve button carries when a role or the authorship is
              // what refuses the action; a phase that simply has nothing to reject says nothing,
              // because that is not a refusal (UI-44).
              disabledReason={own ? undefined : (disabledReason ?? undefined)}
              loading={rejectMutation.isPending}
              onClick={() => setRejecting(true)}
            >
              {rejectMutation.isPending ? t(words.busy) : t(words.action)}
            </Button>
            <Dialog
              open={rejecting}
              onOpenChange={setRejecting}
              title={t(words.title)}
              description={t(words.lead)}
              closeLabel={t("approvals.rejectCancel")}
              footer={
                <>
                  <Button onClick={() => setRejecting(false)}>{t("approvals.rejectCancel")}</Button>
                  <Button
                    variant="danger"
                    disabled={rejectReason.trim() === ""}
                  disabledReason={t("approvals.reasonFirst")}
                    loading={rejectMutation.isPending}
                    onClick={() => {
                      setRejecting(false);
                      rejectMutation.mutate(rejectReason);
                    }}
                  >
                    {t(words.confirm)}
                  </Button>
                </>
              }
            >
              <Field
                id={rejectReasonId}
                label={own ? t("approvals.withdrawReason") : t("approvals.rejectReason")}
                help={own ? t("approvals.withdrawReasonHint") : t("approvals.rejectReasonHint")}
              >
                <Textarea
                  id={rejectReasonId}
                  rows={3}
                  maxLength={2000}
                  required
                  value={rejectReason}
                  onChange={(event) => setRejectReason(event.target.value)}
                />
              </Field>
            </Dialog>

          </div>
        </section>
      ) : null}
    </div>
  );
}

/**
 * The shape of the change while it is being read, rather than one word in the top-left corner
 * (UI-01). The header, the author card and the diff table each keep their height, so nothing
 * on the page moves when the answer lands; the container is what announces the wait.
 */
function ChangeSkeleton({ label }: { label: string }): JSX.Element {
  return (
    <div className="space-y-6" role="status" aria-busy="true" aria-label={label}>
      <div className="space-y-2">
        <Skeleton className="h-7 w-2/3" />
        <Skeleton className="h-4 w-40" />
      </div>
      <div className="grid grid-cols-1 gap-4 rounded border border-border bg-surface-subtle p-4 sm:grid-cols-2">
        <Skeleton className="h-10" />
        <Skeleton className="h-10" />
      </div>
      <Skeleton className="h-40" />
    </div>
  );
}

/** The file writes a CKAN publication or its catalogue description (EP-62, EP-83). */
function publishes(file: ChangeFile): boolean {
  return (file.fields ?? []).some((f) => /^spec\.(publish\.ckan|catalog)\b/.test(f.path));
}

/**
 * The CKAN page the Endpoint becomes once this Change is merged: the live manifest with the
 * file's diff applied, drawn by the publish flow's own preview, so the approver reads what the
 * steward saw (EP-142, T-4847). An Endpoint the Change creates starts from nothing.
 */
type Texts = string | Record<string, string> | null;

function PublicationOnApproval({ project, file }: { project: string; file: ChangeFile }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const name = file.path.split("/").pop()?.replace(/\.ya?ml$/, "") ?? "";
  const live = useQuery({
    queryKey: ["projects", project, "endpoints", name, "manifest"],
    enabled: file.operation === "Update" && name !== "",
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "endpoints", name } },
        }),
      ),
  });
  if (file.operation === "Update" && live.isPending) return null;
  const base = live.data ? { metadata: live.data.metadata, spec: live.data.spec } : {};
  const head = applyFields(base as Record<string, unknown>, file.fields ?? []) as {
    metadata?: { name?: string; title?: Texts; description?: Texts };
    spec?: { catalog?: Record<string, unknown>; audience?: string };
  };
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  return (
    <div className="space-y-2" data-testid="publication-preview">
      <h2 className="text-base font-semibold text-surface-fg">
        {t("approvals.publication", { name })}
      </h2>
      {head.spec?.audience === "public" ? (
        <p className="text-caption text-fg-muted">{t("approvals.publicationPublic")}</p>
      ) : null}
      <PublicationPreview
        label={t("approvals.publication", { name })}
        title={localized(head.metadata?.title, locale, head.metadata?.name ?? name)}
        description={localized(head.metadata?.description, locale, "")}
        catalog={head.spec?.catalog ?? {}}
      />
    </div>
  );
}
