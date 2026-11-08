import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import {
  Alert,
  Card,
  CardHeader,
  EmptyState,
  Icon,
  PageFailed,
  PageLoading,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { Prose } from "../apps/Prose";
import { bytesText } from "../knowledge/knowledge";
import { crumbsOf, useRepositoryRead } from "./api";
import type { FileRead, Repository, Tree } from "./api";
import { RepoLink } from "./links";
import type { RepositorySearch } from "./links";

/** Where the page stands: the repository's root, then each folder of the path, the last unlinked. */
export function Breadcrumb({
  project,
  repository,
  gitRef,
  path,
}: {
  project: string;
  repository: string;
  gitRef?: string;
  path: string;
}): JSX.Element {
  const { t } = useTranslation();
  const crumbs = crumbsOf(path);
  const at = (folder?: string) => ({ tab: "files" as const, ...(gitRef ? { ref: gitRef } : {}), ...(folder ? { path: folder } : {}) });
  return (
    <nav aria-label={t("repositories.files.breadcrumb")} className="min-w-0">
      <ol className="flex min-w-0 flex-wrap items-center gap-1 font-mono text-body">
        <li>
          {crumbs.length === 0 ? (
            <span aria-current="page" className="font-semibold">
              {repository}
            </span>
          ) : (
            <RepoLink project={project} name={repository} search={at()}>
              {repository}
            </RepoLink>
          )}
        </li>
        {crumbs.map((crumb, index) => (
          <li key={crumb.path} className="flex min-w-0 items-center gap-1">
            <span aria-hidden="true" className="text-fg-muted">
              /
            </span>
            {index === crumbs.length - 1 ? (
              <span aria-current="page" className="break-all font-semibold">
                {crumb.name}
              </span>
            ) : (
              <RepoLink project={project} name={repository} search={at(crumb.path)} className="focus-ring break-all rounded-sm text-primary-soft-fg underline-offset-2 hover:underline">
                {crumb.name}
              </RepoLink>
            )}
          </li>
        ))}
      </ol>
    </nav>
  );
}

/** The files tab: a folder's listing and the README at the root, or one file when the address names it. */
export function FilesView({
  project,
  repository,
  search,
}: {
  project: string;
  repository: Repository;
  search: RepositorySearch;
}): JSX.Element {
  const { t } = useTranslation();
  if (repository.empty) {
    return <EmptyState icon="git" title={t("repositories.files.emptyRepository")} description={t("repositories.files.emptyRepositoryHint")} />;
  }
  if (search.file) {
    return <FileView project={project} repository={repository.name} gitRef={search.ref} path={search.file} />;
  }
  return <FolderView project={project} repository={repository} gitRef={search.ref} path={search.path ?? ""} />;
}

export function FolderView({
  project,
  repository,
  gitRef,
  path,
}: {
  project: string;
  repository: Repository;
  gitRef?: string;
  path: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const input = { repository: repository.name, ...(gitRef ? { ref: gitRef } : {}), ...(path ? { path } : {}) };
  const tree = useRepositoryRead<Tree>(project, "jc_repository_tree", input);
  const language = i18n.resolvedLanguage ?? "en";
  const parent = crumbsOf(path).slice(0, -1).at(-1)?.path;
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Breadcrumb project={project} repository={repository.name} gitRef={gitRef} path={path} />
        <span className="text-caption text-fg-muted">
          {t("repositories.files.atRef", { ref: tree.data?.ref ?? gitRef ?? repository.defaultBranch })}
        </span>
      </div>
      {tree.isError ? (
        <PageFailed error={tree.error} onRetry={() => void tree.refetch()} />
      ) : tree.isPending ? (
        <PageLoading label={t("repositories.loading")} />
      ) : tree.data.entries.length === 0 ? (
        <EmptyState icon="git" title={t("repositories.files.emptyFolder")} description={t("repositories.files.emptyFolderHint")} />
      ) : (
        <Table caption={t("repositories.files.caption", { path: path || repository.name })}>
          <TableHead>
            <TableHeaderCell>{t("repositories.files.name")}</TableHeaderCell>
            <TableHeaderCell className="text-right">{t("repositories.files.size")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {path ? (
              <TableRow>
                <TableCell colSpan={2}>
                  <RepoLink
                    project={project}
                    name={repository.name}
                    search={{ tab: "files", ...(gitRef ? { ref: gitRef } : {}), ...(parent ? { path: parent } : {}) }}
                  >
                    {t("repositories.files.up")}
                  </RepoLink>
                </TableCell>
              </TableRow>
            ) : null}
            {tree.data.entries.map((entry) => (
              <TableRow key={entry.path}>
                <TableCell>
                  <span className="flex min-w-0 items-center gap-2">
                    <Icon name={entry.type === "folder" ? "flows" : entry.type === "submodule" ? "link" : "pencil"} className="size-4 shrink-0 text-fg-muted" />
                    {entry.type === "submodule" ? (
                      <span className="break-all font-mono">{entry.name}</span>
                    ) : (
                      <RepoLink
                        project={project}
                        name={repository.name}
                        search={{
                          tab: "files",
                          ...(gitRef ? { ref: gitRef } : {}),
                          ...(entry.type === "folder" ? { path: entry.path } : { file: entry.path }),
                        }}
                        className="focus-ring break-all rounded-sm font-mono text-primary-soft-fg underline-offset-2 hover:underline"
                      >
                        {entry.name}
                      </RepoLink>
                    )}
                  </span>
                </TableCell>
                <TableCell className="whitespace-nowrap text-right text-caption text-fg-muted">
                  {entry.type === "file" ? bytesText(entry.size, language) : "—"}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      {tree.data?.truncated ? <Alert tone="info">{t("repositories.files.truncated")}</Alert> : null}
      {!path && repository.readme ? (
        <Card as="section" aria-labelledby="repository-readme">
          <CardHeader as="h2" titleId="repository-readme" title={repository.readme.path} />
          {/* Markdown drawn as text and checked links, never as markup: a `<script>` or a
              `javascript:` link in a README is characters on the page (AP-53). */}
          <div className="text-body">
            <Prose text={repository.readme.text} />
          </div>
        </Card>
      ) : null}
    </div>
  );
}

/** One file, its lines numbered; a binary or a large file is named and sized instead. */
export function FileView({
  project,
  repository,
  gitRef,
  path,
}: {
  project: string;
  repository: string;
  gitRef?: string;
  path: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const file = useRepositoryRead<FileRead>(project, "jc_repository_file", { repository, path, ...(gitRef ? { ref: gitRef } : {}) });
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Breadcrumb project={project} repository={repository} gitRef={gitRef} path={path} />
        <RepoLink project={project} name={repository} search={{ tab: "commits", ...(gitRef ? { ref: gitRef } : {}), path }}>
          {t("repositories.files.history")}
        </RepoLink>
      </div>
      {file.isError ? (
        <PageFailed error={file.error} onRetry={() => void file.refetch()} />
      ) : file.isPending ? (
        <PageLoading label={t("repositories.loading")} />
      ) : file.data.tooLarge || file.data.binary || file.data.text === undefined ? (
        <EmptyState
          icon="info"
          title={file.data.tooLarge ? t("repositories.file.tooLarge") : t("repositories.file.binary")}
          description={t("repositories.file.notShown", { size: bytesText(file.data.size, i18n.resolvedLanguage ?? "en") })}
        />
      ) : (
        <FileLines text={file.data.text} label={path} />
      )}
    </div>
  );
}

/** A text as numbered lines: React text nodes only, so markup in a file stays characters. */
export function FileLines({ text, label }: { text: string; label: string }): JSX.Element {
  const lines = text.replace(/\n$/, "").split("\n");
  return (
    <Card flush role="group" aria-label={label} tabIndex={0} className="focus-ring overflow-x-auto">
      <div className="table min-w-full py-1 font-mono text-caption">
        {lines.map((line, index) => (
          <div key={index} className="table-row">
            <span aria-hidden="true" className="table-cell select-none border-r border-border pl-3 pr-2 text-right text-fg-muted">
              {index + 1}
            </span>
            <span className="table-cell w-full whitespace-pre px-3">{line || " "}</span>
          </div>
        ))}
      </div>
    </Card>
  );
}
