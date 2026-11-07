/**
 * What inviting a person decides (PF-108, T-3237): the roles an invitation offers, the binding
 * that proposes the chosen one, and where a pending invitation stands. Pure, so each rule is
 * tested without a page.
 */
import type { Manifest } from "../../api/manifest";
import { bindingOf } from "../access/RoleBindings";

/** The roles an invitation offers, least first; each one's description says what it may do. */
export const INVITE_ROLES = ["viewer", "editor", "steward"] as const;
export type InviteRole = (typeof INVITE_ROLES)[number];

/**
 * The RoleBinding that gives the invited person `role` in `project`, naming them by the Keycloak
 * user id the invitation created, never their e-mail (ADR-N-061). It is proposed like every other
 * grant and holds once approved: the invitation itself grants nothing (PF-95, PF-108).
 */
export function inviteBinding(id: string, role: InviteRole, project: string): Manifest {
  // A project place: no Organization is named.
  return bindingOf({ subjectKind: "user", subject: id, role, place: "project", until: "" }, project, undefined);
}

export type InvitationState =
  | { state: "accepted" }
  | { state: "pending"; expires?: Date }
  | { state: "expired"; expires: Date };

/**
 * Where a person's invitation stands: accepted once no step is left; otherwise pending until the
 * link expires, and expired after. An expiry the Portal did not record leaves it pending.
 */
export function invitationState(
  person: { requiredActions: string[]; invitationExpires?: string | null },
  now: Date,
): InvitationState {
  if (person.requiredActions.length === 0) return { state: "accepted" };
  const expires = person.invitationExpires ? new Date(person.invitationExpires) : undefined;
  if (!expires || Number.isNaN(expires.getTime())) return { state: "pending" };
  return expires.getTime() <= now.getTime() ? { state: "expired", expires } : { state: "pending", expires };
}
