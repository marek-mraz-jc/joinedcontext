import type { Identity } from "../../auth/AuthProvider";

/**
 * Whether the signed-in person opened the workspace: its owner is the address commits are signed
 * with, or the username. An owner not known yet, or an empty one, is nobody's, so a person without
 * an email address is never taken for the owner of a workspace still loading.
 */
export function isWorkspaceOwner(identity: Pick<Identity, "email" | "username"> | null | undefined, owner: string | null | undefined): boolean {
  if (!identity || !owner) return false;
  return owner === identity.email || owner === identity.username;
}
