import { useQuery } from "@tanstack/react-query";
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
export type Installation = components["schemas"]["Installation"];
export type SmtpServer = components["schemas"]["SmtpServer"];
export type SmtpTest = components["schemas"]["SmtpTest"];
/** The installation's look (T-7188): jc-core's appearance; the server refuses its dashboards and page texts. */
export type InstallationLook = components["schemas"]["OrganizationAppearance"];

/** The roles a host takes, as the server's `HOST_ROLES` names them. */
export const HOST_ROLES = ["working", "opendata", "api", "signin", "apps", "portal-site", "host"] as const;

/** The states of a registry host, in the order a host passes them. */
export const HOST_STATES = ["proposed", "approved", "dns-ok", "cert-issued", "served", "removed"] as const;

export const operatorKeys = {
  organizations: ["operator", "organizations"] as const,
  hosts: ["operator", "hosts"] as const,
  changes: ["operator", "changes"] as const,
  audit: ["operator", "audit"] as const,
  installation: ["operator", "installation"] as const,
};

/** Each Organization's title by its name, so a list shows "Banskobystrický kraj", not "bbsk". */
export function useOrganizationTitles(): (name: string | null | undefined) => string {
  const list = useQuery({ queryKey: operatorKeys.organizations, queryFn: operatorOrganizations });
  return (name) => (name ? (list.data?.items.find((org) => org.name === name)?.title ?? name) : "");
}

/** A server sentence as the start of a line: "Waiting for …", not "waiting for …". */
export function sentence(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

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

export async function operatorInstallation(): Promise<Installation> {
  return (await door("portal_operator_installation", {})) as Installation;
}

/** One test e-mail to the operator's own address through the saved e-mail server. */
export async function sendTestEmail(): Promise<SmtpTest> {
  return (await doorAction("portal_operator_installation_test_email", {})) as SmtpTest;
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

/** A server timestamp in the reader's language, e.g. `17 Nov 2026, 09:00 UTC`; the audit keeps the seconds. */
export function when(at: string, locale: string, seconds = false): string {
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? at : date.toLocaleString(locale, {
        year: "numeric",
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
        second: seconds ? "2-digit" : undefined,
        timeZoneName: "short",
      });
}
