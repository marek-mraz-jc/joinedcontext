import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Badge, Card, CardHeader, EmptyState, PageFailed, PageLoading } from "../../components/ui";
import { shortSha, useRepositoryRead } from "./api";
import type { Refs, Repository } from "./api";
import { RepoLink } from "./links";

/** The branches, the default first, and the tags; each opens the files at that ref. */
export function BranchesView({ project, repository }: { project: string; repository: Repository }): JSX.Element {
  const { t } = useTranslation();
  const refs = useRepositoryRead<Refs>(project, "jc_repository_refs", { repository: repository.name });
  if (refs.isError) return <PageFailed error={refs.error} onRetry={() => void refs.refetch()} />;
  if (refs.isPending) return <PageLoading label={t("repositories.loading")} />;
  const files = (ref: string) => ({ tab: "files" as const, ref });
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card as="section" aria-labelledby="repository-branches" flush>
        <CardHeader as="h2" titleId="repository-branches" title={t("repositories.branches.title")} className="px-4 pt-4" />
        {refs.data.branches.length === 0 ? (
          <EmptyState bare icon="git" title={t("repositories.branches.empty")} description={t("repositories.branches.emptyHint")} />
        ) : (
          <ul className="divide-y divide-border">
            {refs.data.branches.map((branch) => (
              <li key={branch.name} className="flex flex-wrap items-center gap-2 px-4 py-2.5">
                <RepoLink project={project} name={repository.name} search={files(branch.name)} className="focus-ring break-all rounded-sm font-mono text-primary-soft-fg underline-offset-2 hover:underline">
                  {branch.name}
                </RepoLink>
                {branch.default ? <Badge tone="primary">{t("repositories.branches.default")}</Badge> : null}
                {branch.protected ? <Badge tone="info">{t("repositories.branches.protected")}</Badge> : null}
                <code className="ml-auto font-mono text-caption text-fg-muted">{shortSha(branch.headSha)}</code>
              </li>
            ))}
          </ul>
        )}
      </Card>
      <Card as="section" aria-labelledby="repository-tags" flush>
        <CardHeader as="h2" titleId="repository-tags" title={t("repositories.tags.title")} className="px-4 pt-4" />
        {refs.data.tags.length === 0 ? (
          <EmptyState bare icon="git" title={t("repositories.tags.empty")} description={t("repositories.tags.emptyHint")} />
        ) : (
          <ul className="divide-y divide-border">
            {refs.data.tags.map((tag) => (
              <li key={tag.name} className="flex flex-wrap items-center gap-2 px-4 py-2.5">
                <RepoLink project={project} name={repository.name} search={files(tag.name)} className="focus-ring break-all rounded-sm font-mono text-primary-soft-fg underline-offset-2 hover:underline">
                  {tag.name}
                </RepoLink>
                <code className="ml-auto font-mono text-caption text-fg-muted">{shortSha(tag.targetSha)}</code>
              </li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  );
}
