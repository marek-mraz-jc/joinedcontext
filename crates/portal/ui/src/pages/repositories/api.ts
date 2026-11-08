import { useQuery } from "@tanstack/react-query";
import { ApiError, api, unwrap } from "../../api/client";

/**
 * The repository reads of the Portal registry (`jc_repository_*`, src/ops/repositories.rs): one
 * source for these pages, `POST …/ops/{name}` and MCP. A refusal comes back as the `ApiError` the
 * shared failure states read.
 */
export type RepositoryRole = "configuration" | "project" | "application";

export interface RepositorySummary {
  name: string;
  owner?: string;
  role: RepositoryRole;
  description?: string;
  defaultBranch: string;
  private?: boolean;
  archived?: boolean;
  sizeBytes?: number;
  updatedAt?: string;
}

export interface CommitRow {
  sha: string;
  message?: string;
  author?: string;
  date?: string;
}

export interface Repository extends RepositorySummary {
  cloneUrl?: string;
  empty?: boolean;
  lastCommit?: CommitRow | null;
  readme?: { path: string; text: string };
}

export interface Page<T> {
  items: T[];
  total: number;
  nextCursor?: number;
}

export interface TreeEntry {
  name: string;
  path: string;
  type: "folder" | "file" | "submodule";
  size?: number;
}

export interface Tree {
  repository: string;
  ref: string;
  path: string;
  entries: TreeEntry[];
  truncated: boolean;
}

export interface FileRead {
  repository: string;
  ref: string;
  path: string;
  size: number;
  binary: boolean;
  tooLarge: boolean;
  text?: string;
}

export interface CommitFile {
  path: string;
  status: string;
  binary?: boolean;
  patch?: string;
}

export interface Commit {
  repository: string;
  sha: string;
  message?: string;
  author?: string;
  email?: string;
  date?: string;
  parents?: string[];
  files: CommitFile[];
  truncated: boolean;
}

export interface Refs {
  repository: string;
  branches: { name: string; headSha?: string; protected?: boolean; default?: boolean }[];
  tags: { name: string; targetSha?: string }[];
}

export interface PullRow {
  number: number;
  title: string;
  state: "open" | "closed" | "merged";
  author?: string;
  head?: string;
  base?: string;
  createdAt?: string;
  closedAt?: string | null;
}

export interface Pull extends PullRow {
  repository: string;
  body?: string;
  change?: string;
  files: { path: string; status?: string }[];
  reviews: { author?: string; verdict: string; body?: string; commit?: string }[];
}

export interface RepositorySettings {
  repository: string;
  protection: {
    pattern?: string;
    pushAllowed?: boolean;
    forcePushAllowed?: boolean;
    requiredApprovals?: number;
    blockOnRejectedReviews?: boolean;
    blockOnOutdatedBranch?: boolean;
  }[];
  webhooks: { address: string; active: boolean }[];
  collaborators: { group: string; access?: string }[];
  packages: {
    type?: string;
    name?: string;
    version?: string;
    digest?: string;
    size?: number;
    signed?: boolean;
    createdAt?: string;
  }[];
}

export async function readRepository<T>(project: string, name: string, input: Record<string, unknown>): Promise<T> {
  return (await unwrap(
    await api.POST("/api/v1/projects/{project}/ops/{name}", {
      params: { path: { project, name } },
      body: input,
    }),
  )) as T;
}

/** One read, cached under the project, the operation and its input. */
export function useRepositoryRead<T>(
  project: string,
  name: string,
  input: Record<string, unknown>,
  enabled = true,
) {
  return useQuery({
    queryKey: ["projects", project, "repositories", name, input],
    queryFn: () => readRepository<T>(project, name, input),
    enabled,
  });
}

/** Whether a read was answered 404: for these pages, not there or not open to the caller. */
export function isNotFound(error: unknown): boolean {
  return error instanceof ApiError && error.status === 404;
}

/** A commit's short id, as a forge shows it. */
export function shortSha(sha: string | undefined): string {
  return (sha ?? "").slice(0, 7);
}

/** The folders of a path, each with the path up to it: what the breadcrumb links. */
export function crumbsOf(path: string): { name: string; path: string }[] {
  const parts = path.split("/").filter(Boolean);
  return parts.map((name, at) => ({ name, path: parts.slice(0, at + 1).join("/") }));
}

/** One line of a unified diff, with the numbers it stands at on each side. */
export interface DiffLine {
  kind: "hunk" | "context" | "added" | "removed";
  text: string;
  old?: number;
  new?: number;
}

/** A file's unified patch as lines a page draws, numbered from each hunk header. */
export function diffLines(patch: string): DiffLine[] {
  const lines: DiffLine[] = [];
  let old = 0;
  let next = 0;
  for (const line of patch.replace(/\n$/, "").split("\n")) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (hunk) {
      old = Number(hunk[1]);
      next = Number(hunk[2]);
      lines.push({ kind: "hunk", text: line });
    } else if (line.startsWith("+")) {
      lines.push({ kind: "added", text: line.slice(1), new: next++ });
    } else if (line.startsWith("-")) {
      lines.push({ kind: "removed", text: line.slice(1), old: old++ });
    } else if (line !== "" || lines.length > 0) {
      lines.push({ kind: "context", text: line.slice(1), old: old++, new: next++ });
    }
  }
  return lines;
}
