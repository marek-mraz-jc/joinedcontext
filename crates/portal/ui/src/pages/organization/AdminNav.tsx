import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { useAdministers } from "../../api/permissions";
import { navLinkClass } from "../../components/layout/BrandMark";
import { Icon } from "../../components/ui";

/**
 * Administration's sections, grouped as an administrator looks for them (T-4831, owner
 * 2026-10-20): who has access, what the projects publish, what the installation runs, and the
 * organization's own settings. Every section has its own address, `/organization/{tab}`.
 */
export const ORGANIZATION_GROUPS = [
  { id: "access", tabs: ["people", "members", "roles", "groups", "data-access", "service-accounts", "agents", "connectors"] },
  {
    id: "projects",
    tabs: ["projects", "applications", "endpoints", "mcp-servers", "models", "blueprints", "agentprofiles"],
  },
  { id: "platform", tabs: ["brokers", "environments", "dataspaceparticipants", "health", "costs"] },
  { id: "settings", tabs: ["settings", "setup"] },
] as const;

export type OrganizationTab = (typeof ORGANIZATION_GROUPS)[number]["tabs"][number];

/**
 * The sections of `/organization/{tab}`, in the order of the side navigation's groups
 * (Architecture/09 §14.1, T-4831). Here, not with the page, so the router checks a tab without
 * loading every Administration section with the first page (bundle budget).
 */
export const ORGANIZATION_TABS: readonly OrganizationTab[] = ORGANIZATION_GROUPS.flatMap((group) => group.tabs);

/**
 * The sections an administrator of one Organization other than the installation's opens
 * (ADR-N-058 section 2.4, T-5248): who is bound inside it, the groups those bindings name and its
 * own Organization manifest. The rest is the installation's.
 */
export const OWN_ORGANIZATION_TABS: readonly OrganizationTab[] = ["members", "groups", "settings"];

export function isOrganizationTab(value: string): value is OrganizationTab {
  return (ORGANIZATION_TABS as readonly string[]).includes(value);
}

/**
 * The sidebar of Administration (T-4831): a way back to the project the person came from, then
 * one list per group. It replaces the project navigation, so the switcher no longer suggests the
 * page belongs to a project. Somebody who does not administer the organization sees the way back
 * only: the sections are no links to them, and their pages refuse them anyway.
 */
export function AdminNav({
  project,
  active,
  onNavigate,
}: {
  /** The project the way back opens. */
  project: string;
  /** The section in hand; none on a person's or a group's own page. */
  active?: OrganizationTab;
  onNavigate: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const { known, administers, own } = useAdministers();
  const groups = administers
    ? ORGANIZATION_GROUPS
    : own
      ? ORGANIZATION_GROUPS.map((group) => ({
          id: group.id,
          tabs: group.tabs.filter((tab) => OWN_ORGANIZATION_TABS.includes(tab)),
        })).filter((group) => group.tabs.length > 0)
      : [];
  return (
    <>
      <Link
        to="/projects/$project/home"
        params={{ project }}
        onClick={onNavigate}
        className="focus-ring flex items-center gap-2 rounded-md px-2.5 py-1.5 text-body text-fg-muted hover:bg-surface-subtle hover:text-fg"
      >
        <Icon name="chevronLeft" className="size-4" />
        <span>{t("organization.nav.back")}</span>
      </Link>
      {/* Somebody with no section still reads why the column is empty (T-6423). */}
      {known && groups.length === 0 ? (
        <p className="px-2.5 pt-2 text-caption text-fg-muted">{t("organization.nav.notAdmin")}</p>
      ) : null}
      {groups.map((group) => (
            <div key={group.id} className="flex flex-col gap-1">
              <h2
                id={`admin-nav-${group.id}`}
                className="px-2.5 text-caption font-semibold uppercase tracking-wide text-fg-subtle"
              >
                {t(`organization.group.${group.id}`)}
              </h2>
              <ul aria-labelledby={`admin-nav-${group.id}`} className="flex flex-col gap-0.5">
                {group.tabs.map((tab) => (
                  <li key={tab}>
                    <Link
                      to="/organization/$tab"
                      params={{ tab }}
                      onClick={onNavigate}
                      aria-current={tab === active ? "page" : undefined}
                      className={navLinkClass(tab === active)}
                    >
                      <span className="truncate">{t(`organization.tab.${tab}`)}</span>
                    </Link>
                  </li>
                ))}
              </ul>
            </div>
          ))}
    </>
  );
}
