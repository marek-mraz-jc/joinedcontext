import { door, doorAction } from "../../api/odata";
import type { components } from "../../api/schema";

/**
 * The installation operators' doors (T-7129, T-7140): the host registry, the operators' audit,
 * the Organizations with their ids and the operator Changes. Every handler answers a person of
 * the operators' group with a second factor alone; anybody else reads the server's sentence.
 */
export type OperatorHost = components["schemas"]["OperatorHost"];
export type OperatorHostList = components["schemas"]["OperatorHostList"];
export type OperatorAuditEntry = components["schemas"]["OperatorAuditEntry"];
export type OperatorOrganization = components["schemas"]["OperatorOrganization"];
export type OperatorOrganizationId = components["schemas"]["OperatorOrganizationId"];
export type OperatorChange = components["schemas"]["OperatorChange"];
export type ChangeRequest = components["schemas"]["ChangeRequest"];

/** The roles a host takes, as the server's `HOST_ROLES` names them. */
export const HOST_ROLES = ["working", "opendata", "api", "signin", "apps", "portal-site", "host"] as const;

/** The states of a registry host, in the order a host passes them. */
export const HOST_STATES = ["proposed", "approved", "dns-ok", "cert-issued", "served", "removed"] as const;

export const operatorKeys = {
  organizations: ["operator", "organizations"] as const,
  hosts: ["operator", "hosts"] as const,
  changes: ["operator", "changes"] as const,
  audit: ["operator", "audit"] as const,
};

export async function operatorOrganizations(): Promise<{
  items: OperatorOrganization[];
  ids: OperatorOrganizationId[];
}> {
  return (await door("portal_operator_organizations", {})) as {
    items: OperatorOrganization[];
    ids: OperatorOrganizationId[];
  };
}

export async function operatorHosts(): Promise<OperatorHostList> {
  return (await door("portal_operator_hosts", {})) as OperatorHostList;
}

export async function operatorAudit(): Promise<OperatorAuditEntry[]> {
  return ((await door("portal_operator_audit", {})) as { items: OperatorAuditEntry[] }).items;
}

export async function operatorChanges(): Promise<OperatorChange[]> {
  return ((await door("portal_operator_change_list", {})) as { items: OperatorChange[] }).items;
}

export async function proposeChange(change: ChangeRequest): Promise<OperatorChange> {
  return (await doorAction("portal_operator_change_propose", { body: change })) as OperatorChange;
}

export async function decideChange(id: string, verb: "approve" | "reject"): Promise<OperatorChange> {
  return (await doorAction(
    verb === "approve" ? "portal_operator_change_approve" : "portal_operator_change_reject",
    { id },
  )) as OperatorChange;
}

export type Challenge = components["schemas"]["Challenge"];

export async function requestChallenge(domain: string): Promise<Challenge> {
  return (await doorAction("portal_organization_id_challenge", { body: { domain } })) as Challenge;
}

export async function mintId(domain: string): Promise<unknown> {
  return doorAction("portal_organization_id_mint", { body: { domain } });
}

/** The tone of a host's state badge. */
export function stateTone(state: string): "success" | "neutral" | "info" {
  if (state === "served") return "success";
  if (state === "removed") return "neutral";
  return "info";
}

/** A server timestamp in the reader's language, e.g. `17 Nov 2026, 09:00`; the audit keeps the seconds. */
export function when(at: string, locale: string, seconds = false): string {
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? at : date.toLocaleString(locale, { dateStyle: "medium", timeStyle: seconds ? "medium" : "short" });
}
