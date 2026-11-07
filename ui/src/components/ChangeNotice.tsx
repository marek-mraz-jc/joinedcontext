import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { approvalStanding, expectedNameOf } from "../api/approval";
import { api, queryKeys, unwrap } from "../api/client";
import { useDecision } from "../api/decision";
import { usePermissions } from "../api/permissions";
import { useAuth } from "../auth/AuthProvider";
import { LifecycleBadge } from "./status/LifecycleBadge";
import type { Change } from "../api/manifest";
import { Alert, Button, buttonClass, Field, Input } from "./ui";

/** A red change's approver types the resource's name back (CC-19). */
export function ConfirmName({ expected, value, onChange, disabled, title }: {
  expected: string;
  value: string;
  onChange: (value: string) => void;
  disabled: boolean;
  title?: string;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Field
      id="confirm-resource-name"
      label={t("approvals.confirmLabel")}
      description={
        <>
          {t("approvals.confirmPrompt")} <span className="font-mono font-semibold">{expected}</span>
        </>
      }
    >
      <Input id="confirm-resource-name" value={value} onChange={(e) => onChange(e.target.value)} placeholder={expected} disabled={disabled} title={title} className="max-w-sm" />
    </Field>
  );
}

/**
 * What happens next to a waiting change, where the person proposed it: Approve for someone who
 * may decide it, else who does (PF-58). The API decides on its own; this is the same decision the
 * approval page sends.
 */
export function ChangeDecide({ id, project }: { id: string; project: string }): JSX.Element | null {
  const { t } = useTranslation();
  const { identity } = useAuth();
  const permissions = usePermissions(project);
  const decision = useDecision(project, id);
  const [typed, setTyped] = useState("");
  // The same query that follows the change beside this control, so the two never disagree.
  const { data: change } = useQuery({
    queryKey: queryKeys.change(project, id),
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/changes/{id}", { params: { path: { project, id } } })),
  });
  if (change?.status.phase !== "PendingApproval" || change.summary === undefined) return null;
  const standing = approvalStanding(permissions, identity?.email ?? undefined, change);
  if (standing.block !== null) {
    return (
      <div role="status" className="flex flex-col gap-1">
        <p className="flex items-center gap-2 text-body font-medium text-fg">
          <span aria-hidden="true" className="size-2 shrink-0 animate-pulse rounded-full bg-accent motion-reduce:animate-none" />
          {t("changes.waitingApprover")}
        </p>
        <p className="text-caption text-fg-muted">{t("changes.whoApproves")}</p>
      </div>
    );
  }
  const expected = expectedNameOf(change);
  const red = change.status.lane === "red";
  const typedRight = !red || typed.trim() === expected;
  return (
    <div className="flex flex-col gap-3">
      {red ? <ConfirmName expected={expected} value={typed} onChange={setTyped} disabled={decision.approve.isPending} /> : null}
      <Button
        variant="primary"
        className="w-fit"
        disabled={!typedRight}
        disabledReason={typedRight ? undefined : t("approvals.confirmFirst", { name: expected })}
        loading={decision.approve.isPending}
        onClick={() => decision.approve.mutate(red ? typed : undefined)}
      >
        {t("approvals.approve")}
      </Button>
      {decision.error ? (
        <Alert role="alert" tone="danger">
          {decision.error}
        </Alert>
      ) : null}
    </div>
  );
}

/**
 * What a write leaves behind: a merge request waiting for an approver, not a saved record, or one
 * approved as it was proposed because the person administers its kind (PF-58).
 */
export function ChangeNotice({
  change,
  project,
  plain = false,
}: {
  change: Change;
  project: string;
  /** For a person who follows one file: the words alone, the id and lane behind the link. */
  plain?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  // Approved already when it is on its way to the branch or there (PF-58); anything else waits.
  const waiting = !["Deploying", "Merged", "Applied"].includes(change.status.phase);
  return (
    <Alert
      role="status"
      tone="info"
      actions={
        <Link
          to="/projects/$project/approvals/$id"
          params={{ project, id: change.metadata.name }}
          className={buttonClass("secondary", "sm")}
        >
          {waiting ? t("changes.review") : t("changes.open")}
        </Link>
      }
    >
      <span className="flex flex-wrap items-center gap-2">
        <span>{waiting ? t("changes.accepted") : t("changes.approvedAsProposed")}</span>
        {plain ? null : <span className="font-mono font-medium">{change.metadata.name}</span>}
        {/* The lane says who must approve: once approved, red beside Merged only reads as a failure. */}
        {waiting && !plain ? <LifecycleBadge kind="lane" value={change.status.lane} /> : null}
        {plain ? null : <LifecycleBadge kind="phase" value={change.status.phase} />}
      </span>
    </Alert>
  );
}
