import { useMemo } from "react";
import { queryOptions, useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import { api, queryKeys, unwrap } from "./client";
import { asManifests, localized, ORG_NAMESPACE } from "./manifest";
import type { Manifest } from "./manifest";
import { useTranslation } from "react-i18next";
import { offeredLocales, useBranding } from "../branding";

/** The one `GET /api/v1/projects` query every project picker reads, each through its own `select`. */
export const projectsQuery = queryOptions({
  queryKey: queryKeys.projects(),
  queryFn: async () => unwrap(await api.GET("/api/v1/projects")),
});

/**
 * The one `GET /api/v1/projects/{project}` query (its usage, its repository): the quota card, the
 * delete dialog and the export each `select` from it rather than read it under keys of their own.
 * A caller who may not read the project is answered 404, which is not asked again.
 */
export const projectDetailQuery = (project: string) =>
  queryOptions({
    queryKey: [...queryKeys.projects(), project, "detail"],
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}", { params: { path: { project } } })),
  });

/** The projects the configuration repository holds, as `GET /api/v1/projects` lists them (PF-05). */
export function useProjects() {
  return useQuery({
    ...projectsQuery,
    select: (list) => list.items.map((item) => item.name),
  });
}

/**
 * The Organizations the person sees, when they see more than one, and the Organization of each
 * project they read (PF-124, ADR-N-058 section 2.7). The server lists only Organizations owning a
 * project the person reads. Same request as [`useProjects`].
 */
export function useOrganizations() {
  return useQuery({
    ...projectsQuery,
    select: (list) => ({
      organizations: list.organizations ?? [],
      ownerOf: new Map(list.items.map((item) => [item.name, item.organization])),
    }),
  });
}

/**
 * The Organization `?org=` names when it is one the person sees among two or more (PF-124,
 * ADR-N-058 section 2.7), else `undefined`: what the Administration lists show (T-5248).
 */
export function useChosenOrganizationName(): string | undefined {
  const organizations = useOrganizations().data?.organizations ?? [];
  const asked = useRouterState({ select: (state) => (state.location.search as { org?: string }).org });
  return asked !== undefined && organizations.length > 1 && organizations.some((o) => o.name === asked)
    ? asked
    : undefined;
}

/**
 * The sample projects among those the person may read (PF-109): real open data to look around in,
 * marked by the `joinedcontext.com/sample` label on their Project manifest. Same request as
 * [`useProjects`], so it costs nothing more.
 */
export function useSampleProjects() {
  return useQuery({
    ...projectsQuery,
    select: (list) => list.items.filter((item) => item.sample).map((item) => item.name),
  });
}

/**
 * The title a person reads for each project, in the page's language, the slug where its Project
 * manifest names none (T-4243). Same request as [`useProjects`].
 */
export function useProjectTitles(): (project: string) => string {
  const { i18n } = useTranslation();
  const titles = useQuery({
    ...projectsQuery,
    select: (list) => new Map(list.items.map((item) => [item.name, item.title])),
  }).data;
  return (project) => localized(titles?.get(project), i18n.language, project);
}

/**
 * The name of the Organization `project` belongs to (ADR-N-058): its `organizationRef`, as the
 * project list answers it; for the organization's own pages the one chosen in the switcher; and
 * the only Organization of an installation that has one. `undefined` while that is not known.
 */
export function organizationNameOf(
  list: { organizations: { name: string }[]; ownerOf: Map<string, string | null | undefined> } | undefined,
  project: string,
  chosen?: string,
): string | undefined {
  if (!list) return undefined;
  const owner = list.ownerOf.get(project);
  if (owner) return owner;
  if (chosen) return chosen;
  return list.organizations.length === 1 ? list.organizations[0]?.name : undefined;
}

/** The name of the Organization `project` belongs to; see [`organizationNameOf`]. */
export function useOrganizationName(project: string): string | undefined {
  const list = useOrganizations().data;
  const chosen = useChosenOrganizationName();
  return organizationNameOf(list, project, chosen);
}

/**
 * The `Organization` manifest `project` belongs to (ADR-N-058), never merely the first of the
 * list: with several Organizations the first is another body's (F-UI1-011, F-UI1-012).
 * `settled` once both the list and the owner are known, found or not.
 */
export function useProjectOrganization(project: string): { organization: Manifest | undefined; settled: boolean } {
  const owners = useOrganizations();
  const name = organizationNameOf(owners.data, project);
  // Organizations live in the organization's own namespace.
  const organizations = useQuery({
    queryKey: queryKeys.list(ORG_NAMESPACE, "organizations"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project: ORG_NAMESPACE, plural: "organizations" } },
        }),
      ),
  });
  const items = asManifests(organizations.data?.items ?? []);
  const organization = name !== undefined ? items.find((item) => item.metadata.name === name) : items.length === 1 ? items[0] : undefined;
  return { organization, settled: !owners.isPending && !organizations.isPending };
}

/**
 * The organization's domain, in the order the API itself resolves it (`api::assistant::org_domain`):
 * the project's `Organization` manifest, then the installation's branding, then the project name.
 * A model minted under a fabricated `<project>.sk` names IRIs nobody owns (T-0794).
 */
export function useOrgDomain(project: string): string {
  const branding = useBranding();
  const domain = (useProjectOrganization(project).organization?.spec as { domain?: string } | undefined)?.domain;
  return domain || branding.orgDomain || project;
}

/**
 * The languages a model's titles are written in (T-4829, DM-15): the project's Organization's
 * `locales`, else what the installation offers.
 */
export function useOrgLocales(project: string): string[] {
  const branding = useBranding();
  const locales = (useProjectOrganization(project).organization?.spec as { locales?: unknown } | undefined)?.locales;
  // One string, so the list keeps its identity while the answer stands still.
  const own = Array.isArray(locales)
    ? locales.filter((one): one is string => typeof one === "string" && /^[a-z]{2,3}$/.test(one)).join(",")
    : "";
  return useMemo(() => (own !== "" ? own.split(",") : offeredLocales(branding)), [own, branding]);
}

/** Where the last project a person worked in is remembered, in this browser only (T-2753). */
const LAST_PROJECT = "jc.lastProject";

/**
 * Remember the project a page was drawn for, so a page that belongs to no project (the
 * organization's tabs, every endpoint, `/`) opens on it instead of on the first project in the
 * list. Storage that is blocked or full costs the preference, never the page.
 */
export function rememberProject(project: string): void {
  try {
    window.localStorage.setItem(LAST_PROJECT, project);
  } catch {
    // A private window or a blocked store: the first project is the fallback.
  }
}

/** The project last worked in, whether or not the list of projects has come back (T-4165). */
export function lastProject(): string | undefined {
  try {
    return window.localStorage.getItem(LAST_PROJECT) ?? undefined;
  } catch {
    return undefined;
  }
}

/**
 * The project a page that belongs to none opens on: the one last worked in while the person may
 * still read it (T-2753), else the first they may read that is not a sample, else the first
 * (T-5603). The list is the server's, so only a project the person reads is a candidate; a sample
 * is readable to newcomers too, so a first sign-in lands in the person's own project.
 */
export function preferredProject(
  projects: readonly string[] | undefined,
  samples: readonly string[] = [],
): string | undefined {
  if (!projects || projects.length === 0) {
    return undefined;
  }
  const last = lastProject();
  if (last !== undefined && projects.includes(last)) {
    return last;
  }
  return projects.find((project) => !samples.includes(project)) ?? projects[0];
}

/** [`preferredProject`] over the server's list of projects and its samples (T-5603). */
export function usePreferredProject(): string | undefined {
  const projects = useProjects().data;
  const samples = useSampleProjects().data;
  return preferredProject(projects, samples);
}

/** Where the last address in each project is remembered, in this browser only (T-5603). */
const LAST_ADDRESS = "jc.lastAddress";

function addresses(): Record<string, string> {
  try {
    const stored: unknown = JSON.parse(window.localStorage.getItem(LAST_ADDRESS) ?? "{}");
    return stored !== null && typeof stored === "object" && !Array.isArray(stored)
      ? (stored as Record<string, string>)
      : {};
  } catch {
    return {};
  }
}

/** Whether `href` is a page of `project`: a path of this Portal, never another origin. */
function isPageOf(project: string, href: unknown): href is string {
  return typeof href === "string" && href.startsWith(`/projects/${encodeURIComponent(project)}/`) && !href.includes("//");
}

/**
 * Remember the page in hand as the last one in its project, so the project switcher reopens the
 * space and type a person left there (T-5603). Storage that is blocked or full costs the
 * preference, never the page.
 */
export function rememberAddress(project: string, href: string): void {
  if (!isPageOf(project, href)) {
    return;
  }
  try {
    window.localStorage.setItem(LAST_ADDRESS, JSON.stringify({ ...addresses(), [project]: href }));
  } catch {
    // A private window or a blocked store: the switch keeps the section only.
  }
}

/**
 * The last page a person had open in `project`, when one is remembered. The page it opens checks
 * the person's rights as any address does; a project they no longer read is not in the switcher.
 */
export function lastAddress(project: string): string | undefined {
  const href = addresses()[project];
  return isPageOf(project, href) ? href : undefined;
}
