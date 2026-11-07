import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Badge } from "../ui";
import type { BadgeTone } from "../ui";

export type LifecycleKind = "lane" | "phase" | "appLifecycle";

export interface LifecycleBadgeProps {
  kind: LifecycleKind;
  value?: string | null;
  className?: string;
}

/** Lower-cased status value → the label key under `lane.`/`phase.` and its chip tone. */
const LANES: Record<string, [string, BadgeTone]> = {
  green: ["green", "success"],
  yellow: ["yellow", "warning"],
  red: ["red", "danger"],
};

const PHASES: Record<string, [string, BadgeTone]> = {
  draft: ["draft", "neutral"],
  // A resource's `Pending` is a merged manifest the reconciler has not deployed (over a quota,
  // paused, a compute it does not run); only a Change is ever waiting for an approver, and
  // reading it as one sent people to an Approvals page with nothing on it (T-2873, CC-33).
  pending: ["notDeployed", "warning"],
  pendingapproval: ["pendingApproval", "warning"],
  deploying: ["deploying", "info"],
  live: ["live", "success"],
  merged: ["merged", "success"],
  applied: ["applied", "success"],
  error: ["error", "danger"],
  rejected: ["rejected", "danger"],
  drifted: ["drifted", "accent"],
  // What a `SyncSource` reports about its own loop (MF-30). `pendingapproval` and `error`
  // above mean the same thing for it, so only the three it adds are here.
  synced: ["synced", "success"],
  outofsync: ["outOfSync", "warning"],
  paused: ["paused", "neutral"],
};

/** An app's own lifecycle, which is not the reconciler's phase (AP-18). */
const APP_LIFECYCLE: Record<string, [string, BadgeTone]> = {
  draft: ["draft", "neutral"],
  preview: ["preview", "warning"],
  published: ["published", "success"],
  retired: ["retired", "neutral"],
};

/**
 * One chip for both halves of a resource's lifecycle: the risk lane a change falls into
 * and the phase the reconciler has it in. The tooltip carries the explanation, so the
 * colour is never the only carrier of meaning.
 */
export function LifecycleBadge({ kind, value, className }: LifecycleBadgeProps): JSX.Element {
  const { t } = useTranslation();
  const raw = value ?? "";
  const vocabulary = kind === "lane" ? LANES : kind === "appLifecycle" ? APP_LIFECYCLE : PHASES;
  const entry = vocabulary[raw.toLowerCase()];

  return (
    <Badge
      tone={entry ? entry[1] : "neutral"}
      title={entry ? t(`${kind}.${entry[0]}Help`) : undefined}
      className={className}
    >
      <span aria-hidden="true" className="size-1.5 rounded-full bg-current opacity-70" />
      {entry ? t(`${kind}.${entry[0]}`) : raw}
    </Badge>
  );
}
