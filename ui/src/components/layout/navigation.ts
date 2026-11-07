import type { IconName } from "../ui";

/** The plural segments of `/api/v1/projects/{project}/{plural}`, in sidebar order. */
export const NAV_SECTIONS = [
  // The project's home: the first-run checklist and what the person's role does next (T-3233).
  { plural: "home", labelKey: "nav.home", icon: "globe" },
  { plural: "spaces", labelKey: "nav.spaces", icon: "spaces" },
  // What this project references from other projects' endpoints are rows of the Endpoints grid,
  // not a page of their own (T-0706, EP-15, T-6169).
  { plural: "endpoints", labelKey: "nav.endpoints", icon: "endpoints" },
  // A standing query of a space and where its notifications go; like a sync source, its page
  // is the only way to add one, so it needs an entry of its own (CC-72, T-2344).
  { plural: "subscriptions", labelKey: "nav.subscriptions", icon: "inbox" },
  // A source is what a pipeline reads, so it sits in front of the pipelines (MF-35).
  { plural: "datasources", labelKey: "nav.datasources", icon: "datasources" },
  // Context sources a hub space's broker forwards to: a member's Endpoint or an external broker
  // (MF-36, PF-48). Its list had no entry and was reachable only by its address (T-4819).
  { plural: "csrs", labelKey: "nav.csrs", icon: "link" },
  // The project's MQTT brokers, which a data source reads and a device publishes to (MF-54, T-3839).
  { plural: "mqttbrokers", labelKey: "nav.mqttbrokers", icon: "datasources" },
  { plural: "pipelines", labelKey: "nav.pipelines", icon: "pipelines" },
  { plural: "dashboards", labelKey: "nav.dashboards", icon: "dashboards" },
  // A person's charts over an Endpoint, saved into the project's Grafana folder (T-6509).
  { plural: "grafana", labelKey: "nav.grafana", icon: "dashboards" },
  { plural: "apps", labelKey: "nav.apps", icon: "apps" },
  // Where this project's resources are copied or mirrored from; the page that adds one is the
  // only way into it, so it needs an entry of its own (MF-27, T-0790).
  { plural: "syncsources", labelKey: "nav.sync", icon: "refresh" },
  { plural: "assistant", labelKey: "nav.assistant", icon: "chat" },
  // Copies of the project a person changes on the side and brings back as one Change (UI-61).
  { plural: "workspaces", labelKey: "nav.workspaces", icon: "git" },
  { plural: "approvals", labelKey: "nav.approvals", icon: "approvals" },
  // What the reconciler, the pipelines, the gateway, the broker and the catalogue did, in one
  // place, so "is it working" has an answer that is not a Grafana login (UI-31).
  { plural: "activity", labelKey: "nav.activity", icon: "refresh" },
  // What is broken in the project and how to fix it, one row per check of a resource (T-5067).
  { plural: "health", labelKey: "nav.health", icon: "warning" },
  // Who may read and write the context data, beside the platform roles that say who may
  // change the manifests: a policy is authored like every other kind (T-2326, R5).
  { plural: "policies", labelKey: "nav.policies", icon: "access" },
  // The last item of the project's menu: its title and quotas, who is bound in it, its roles, its
  // service accounts and its deletion. Project → Access folded into it (T-2606, §14.3).
  { plural: "settings", labelKey: "nav.settings", icon: "access" },
] as const satisfies ReadonlyArray<{ plural: string; labelKey: string; icon: IconName }>;

/**
 * The sections an installation may hide (`hiddenSections` of the branding, T-2874). Until the
 * branding has answered they are treated as hidden, so neither the menu nor a bookmarked address
 * shows one for a moment and then takes it away.
 */
export const HIDEABLE_SECTIONS: readonly string[] = ["dashboards"];

/** Whether `plural` is hidden, given what the branding said (`undefined`: not yet). */
export function isHiddenSection(plural: string, hidden: readonly string[] | undefined): boolean {
  return hidden === undefined ? HIDEABLE_SECTIONS.includes(plural) : hidden.includes(plural);
}

/** Where the project menu goes: the same section of the other project, never past its list. */
export interface SameSection {
  /** The `$plural` segment of `/projects/$project/$plural`. */
  plural: string;
}

/**
 * The section of the page in hand, so switching project keeps it (T-2425, UI-05).
 *
 * A list route is its own section and stays where it is. A route that names one resource —
 * `/endpoints/air-quality`, `/approvals/chg-2a`, `/workspaces/air-v2/compare` — falls back to
 * that section's list in the other project, because the name belongs to the project that was
 * left. A path that names no section at all lands on Spaces, which is where the menu used to
 * send every switch.
 *
 * Nothing of the old URL travels but the section. Every search key a route here takes —
 * `space`, `endpoint`, `entityId`, `edit`, `workspace` — names a resource of the project being
 * left, and a name carried into another project's URL is a 404 at best and a different resource
 * with the same name at worst. The two that are filters rather than names (`q`, `type`) belong
 * to Explore, whose other half is a space and an endpoint of the old project, so they have
 * nothing to narrow on the other side; and every list route drops the search it does not
 * declare in any case.
 */
export function sameSection(pathname: string): SameSection {
  const [, , section] = pathname.split("/").filter(Boolean);
  // A segment that is not a plural is not a section: a stray path lands on Spaces rather than
  // building `/projects/other/%2E%2E`.
  return { plural: section && /^[a-z][a-z0-9-]*$/.test(section) ? section : "spaces" };
}
