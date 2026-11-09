import type { JSX, ReactNode } from "react";
import { Link } from "@tanstack/react-router";

export const REPOSITORY_TABS = ["files", "commits", "branches", "pulls", "settings"] as const;
export type RepositoryTab = (typeof REPOSITORY_TABS)[number];

/** What the address of a repository's page holds: the view and what it shows. */
export interface RepositorySearch {
  tab?: RepositoryTab;
  ref?: string;
  path?: string;
  file?: string;
  commit?: string;
  pull?: number;
}

/** The search of the address, read the way the route validates it: anything else is dropped. */
export function repositorySearch(search: Record<string, unknown>): RepositorySearch {
  const text = (value: unknown) => (typeof value === "string" && value.length > 0 && value.length <= 1024 ? value : undefined);
  const tab = REPOSITORY_TABS.find((value) => value === search.tab);
  const pull = Number(search.pull);
  return {
    ...(tab ? { tab } : {}),
    ...(text(search.ref) ? { ref: text(search.ref) } : {}),
    ...(text(search.path) ? { path: text(search.path) } : {}),
    ...(text(search.file) ? { file: text(search.file) } : {}),
    ...(text(search.commit) && /^[0-9a-f]{40}$/.test(search.commit as string) ? { commit: search.commit as string } : {}),
    ...(Number.isInteger(pull) && pull > 0 ? { pull } : {}),
  };
}

/** A link to another view of the same repository's page. */
export function RepoLink({
  project,
  name,
  search,
  className,
  children,
}: {
  project: string;
  name: string;
  search: RepositorySearch;
  className?: string;
  children: ReactNode;
}): JSX.Element {
  return (
    <Link
      to="/projects/$project/repositories/$name"
      params={{ project, name }}
      search={search}
      className={className ?? "focus-ring rounded-sm text-primary-soft-fg underline-offset-2 hover:underline"}
    >
      {children}
    </Link>
  );
}

