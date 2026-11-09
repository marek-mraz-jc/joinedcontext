import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Alert, Badge, Card, EmptyState, PageFailed, PageLoading } from "../../components/ui";
import { Prose } from "../apps/Prose";
import { diffLines, shortSha, useRepositoryRead } from "./api";
import type { Commit, CommitFile, CommitRow, Page, Repository } from "./api";
import { RepoLink } from "./links";
import type { RepositorySearch } from "./links";
import { Pager } from "./RepositoriesPage";

const PAGE = 30;

/** When a commit was made, in the person's locale. */
export function when(iso: string | undefined, language: string): string {
  if (!iso) return "—";
  const at = Date.parse(iso);
  return Number.isNaN(at) ? iso : new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "short" }).format(at);
}

/** The commits tab: the list at a ref (of one path when the address names it), or one commit. */
export function CommitsView({
  project,
  repository,
  search,
}: {
  project: string;
  repository: Repository;
  search: RepositorySearch;
}): JSX.Element {
  const { t } = useTranslation();
  if (search.commit) {
    return <CommitDetail project={project} repository={repository.name} sha={search.commit} />;
  }
  if (repository.empty) {
    return <EmptyState icon="git" title={t("repositories.files.emptyRepository")} description={t("repositories.files.emptyRepositoryHint")} />;
  }
  return <CommitList project={project} repository={repository} gitRef={search.ref} path={search.path} />;
}

export function CommitList({
  project,
  repository,
  gitRef,
  path,
}: {
  project: string;
  repository: Repository;
  gitRef?: string;
  path?: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const [cursor, setCursor] = useState(0);
  const input = {
    repository: repository.name,
    ...(gitRef ? { ref: gitRef } : {}),
    ...(path ? { path } : {}),
    cursor,
    limit: PAGE,
  };
  const commits = useRepositoryRead<Page<CommitRow> & { ref?: string }>(project, "jc_repository_commits", input);
  const language = i18n.resolvedLanguage ?? "en";
  if (commits.isError) return <PageFailed error={commits.error} onRetry={() => void commits.refetch()} />;
  if (commits.isPending) return <PageLoading label={t("repositories.loading")} />;
  return (
    <div className="space-y-3">
      {path ? (
        <RepoLink project={project} name={repository.name} search={{ tab: "commits", ...(gitRef ? { ref: gitRef } : {}) }}>
          {t("repositories.commits.all")}
        </RepoLink>
      ) : null}
      <p className="text-caption text-fg-muted">
        {path
          ? t("repositories.commits.ofPath", { path, ref: commits.data.ref ?? repository.defaultBranch })
          : t("repositories.commits.atRef", { ref: commits.data.ref ?? repository.defaultBranch })}
      </p>
      {commits.data.items.length === 0 ? (
        <EmptyState icon="git" title={t("repositories.commits.empty")} description={t("repositories.commits.emptyHint")} />
      ) : (
        <Card as="ul" flush className="divide-y divide-border overflow-hidden">
          {commits.data.items.map((commit) => (
            <li key={commit.sha} className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 px-4 py-3">
              <div className="min-w-0">
                <RepoLink
                  project={project}
                  name={repository.name}
                  search={{ tab: "commits", commit: commit.sha }}
                  className="focus-ring break-words rounded-sm font-semibold text-primary-soft-fg underline-offset-2 hover:underline"
                >
                  {commit.message || t("repositories.commits.noMessage")}
                </RepoLink>
                <p className="text-caption text-fg-muted">
                  {t("repositories.commits.by", { author: commit.author ?? "—", date: when(commit.date, language) })}
                </p>
              </div>
              <code className="font-mono text-caption text-fg-muted">{shortSha(commit.sha)}</code>
            </li>
          ))}
        </Card>
      )}
      <Pager total={commits.data.total} cursor={cursor} size={PAGE} next={commits.data.nextCursor} onCursor={setCursor} />
    </div>
  );
}

const STATUS_TONE: Record<string, "success" | "danger" | "info" | "neutral"> = {
  added: "success",
  deleted: "danger",
  renamed: "info",
  modified: "neutral",
};

/** A changed file's status in words. */
export function StatusBadge({ status }: { status?: string }): JSX.Element {
  const { t } = useTranslation();
  const known = status && status in STATUS_TONE ? status : "modified";
  return <Badge tone={STATUS_TONE[known]}>{t(`repositories.status.${known}`)}</Badge>;
}

export function CommitDetail({ project, repository, sha }: { project: string; repository: string; sha: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const commit = useRepositoryRead<Commit>(project, "jc_repository_commit", { repository, sha });
  if (commit.isError) return <PageFailed error={commit.error} onRetry={() => void commit.refetch()} />;
  if (commit.isPending) return <PageLoading label={t("repositories.loading")} />;
  const data = commit.data;
  const [subject, ...rest] = (data.message ?? "").split("\n");
  return (
    <div className="space-y-4">
      <RepoLink project={project} name={repository} search={{ tab: "commits" }}>
        {t("repositories.commits.all")}
      </RepoLink>
      <Card as="section" aria-labelledby="commit-subject" className="space-y-2">
        <h2 id="commit-subject" className="break-words text-title font-semibold">
          {subject || t("repositories.commits.noMessage")}
        </h2>
        {rest.join("\n").trim() ? (
          <div className="text-body">
            <Prose text={rest.join("\n").trim()} />
          </div>
        ) : null}
        <p className="text-caption text-fg-muted">
          {t("repositories.commits.by", { author: data.author ?? "—", date: when(data.date, i18n.resolvedLanguage ?? "en") })}
        </p>
        <p className="flex flex-wrap gap-x-4 gap-y-1 font-mono text-caption">
          <span>
            {t("repositories.commits.sha")} {data.sha}
          </span>
          {(data.parents ?? []).map((parent) => (
            <span key={parent}>
              {t("repositories.commits.parent")}{" "}
              <RepoLink project={project} name={repository} search={{ tab: "commits", commit: parent }}>
                {shortSha(parent)}
              </RepoLink>
            </span>
          ))}
        </p>
      </Card>
      <p className="text-body">{t("repositories.commits.files", { count: data.files.length })}</p>
      {data.files.map((file) => (
        <FileDiff key={file.path} file={file} />
      ))}
      {data.truncated ? <Alert tone="info">{t("repositories.commits.truncated")}</Alert> : null}
    </div>
  );
}

/** One file of a commit: its path and status, then its patch as numbered lines. */
export function FileDiff({ file }: { file: CommitFile }): JSX.Element {
  const { t } = useTranslation();
  return (
    <Card as="section" flush aria-label={file.path} className="overflow-hidden">
      <header className="flex flex-wrap items-center gap-2 border-b border-border bg-surface-subtle px-3 py-2">
        <StatusBadge status={file.status} />
        <span className="min-w-0 break-all font-mono text-caption font-semibold">{file.path}</span>
      </header>
      {file.binary ? (
        <p className="px-3 py-2 text-caption text-fg-muted">{t("repositories.diff.binary")}</p>
      ) : file.patch === undefined ? (
        <p className="px-3 py-2 text-caption text-fg-muted">{t("repositories.diff.notShown")}</p>
      ) : file.patch === "" ? (
        <p className="px-3 py-2 text-caption text-fg-muted">{t("repositories.diff.noChange")}</p>
      ) : (
        <DiffView patch={file.patch} label={file.path} />
      )}
    </Card>
  );
}

const LINE_CLASS = {
  hunk: "bg-info-soft text-fg-muted",
  context: "",
  added: "bg-success-soft",
  removed: "bg-danger-soft",
} as const;

/** A unified patch, its lines numbered on both sides and marked; it scrolls inside itself on a phone. */
export function DiffView({ patch, label }: { patch: string; label: string }): JSX.Element {
  const { t } = useTranslation();
  const lines = diffLines(patch);
  return (
    <div role="group" aria-label={t("repositories.diff.label", { path: label })} tabIndex={0} className="focus-ring overflow-x-auto">
      <div className="table min-w-full font-mono text-caption">
        {lines.map((line, index) =>
          line.kind === "hunk" ? (
            <div key={index} className={`table-row ${LINE_CLASS.hunk}`}>
              <span aria-hidden="true" className="table-cell" />
              <span aria-hidden="true" className="table-cell border-r border-border" />
              <span className="table-cell whitespace-pre px-2 py-0.5">{line.text}</span>
            </div>
          ) : (
            <div key={index} className={`table-row ${LINE_CLASS[line.kind]}`}>
              <span aria-hidden="true" className="table-cell select-none pl-2 pr-1 text-right text-fg-muted">
                {line.old ?? ""}
              </span>
              <span aria-hidden="true" className="table-cell select-none border-r border-border pl-2 pr-1 text-right text-fg-muted">
                {line.new ?? ""}
              </span>
              <span className="table-cell w-full whitespace-pre px-2">
                <span aria-label={line.kind === "added" ? t("repositories.diff.added") : line.kind === "removed" ? t("repositories.diff.removed") : undefined}>
                  {line.kind === "added" ? "+" : line.kind === "removed" ? "-" : " "}
                </span>
                {line.text}
              </span>
            </div>
          ),
        )}
      </div>
    </div>
  );
}
