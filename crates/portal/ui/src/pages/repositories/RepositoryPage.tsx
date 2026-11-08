import { useState } from "react";
import type { JSX } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { usePermissions } from "../../api/permissions";
import { Button, Icon, PageFailed, PageHeader, PageLoading, Tabs, tabPanelProps } from "../../components/ui";
import { isNotFound, useRepositoryRead } from "./api";
import { REPOSITORY_TABS } from "./links";
import type { RepositorySearch } from "./links";
import type { Repository } from "./api";
import { RoleBadge } from "./RepositoriesPage";
import { FilesView } from "./FilesView";
import { CommitsView } from "./CommitsView";
import { BranchesView } from "./BranchesView";
import { PullsView } from "./PullsView";
import { SettingsView } from "./SettingsView";

/** The clone address with a copy button: what a git client is given. */
export function CloneAddress({ url }: { url: string }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      <span className="text-caption text-fg-muted">{t("repositories.clone.label")}</span>
      <code className="min-w-0 max-w-full truncate rounded-sm bg-surface-subtle px-1.5 py-0.5 font-mono text-caption">{url}</code>
      <Button
        size="sm"
        variant="ghost"
        aria-label={t("repositories.clone.copy")}
        icon={<Icon name={copied ? "check" : "copy"} className="size-4" />}
        onClick={() => {
          void navigator.clipboard?.writeText(url).then(() => setCopied(true));
        }}
      >
        {copied ? t("repositories.clone.copied") : t("repositories.clone.copyShort")}
      </Button>
    </div>
  );
}

/**
 * One repository of the project: its files with a breadcrumb and the README, its commits with
 * their diff, its branches and tags, its pull requests, and for whoever approves the project its
 * settings. Every view is in the address, so a link opens the same file, commit or pull request.
 */
export function RepositoryPage({
  project,
  name,
  search,
}: {
  project: string;
  name: string;
  search: RepositorySearch;
}): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const permissions = usePermissions(project);
  const repository = useRepositoryRead<Repository>(project, "jc_repository_get", { repository: name });
  // Only once the document is read: `can` says yes while it is on its way, and what only an
  // approver reads is not offered on a guess (the server answers 403 to anyone else).
  const administers = Boolean(permissions.data) && permissions.can("Project", "approve");
  const tabs = REPOSITORY_TABS.filter((tab) => tab !== "settings" || administers);
  const tab = tabs.find((value) => value === search.tab) ?? "files";
  const back = (
    <Link to="/projects/$project/repositories" params={{ project }} className="focus-ring rounded-sm underline">
      {t("repositories.back")}
    </Link>
  );

  if (repository.isError) {
    return (
      <section aria-labelledby="repository-title" className="space-y-6">
        <PageHeader title={<span id="repository-title" className="font-mono">{name}</span>} description={t("repositories.refused.subtitle")} />
        <PageFailed error={repository.error} onRetry={() => void repository.refetch()} back={back}>
          {isNotFound(repository.error) ? t("repositories.refused.repository", { name, project }) : undefined}
        </PageFailed>
      </section>
    );
  }
  const data = repository.data;
  return (
    <section aria-labelledby="repository-title" className="min-w-0 space-y-5">
      <PageHeader
        title={<span id="repository-title" className="font-mono">{name}</span>}
        description={data?.description || t("repositories.page.intro")}
        breadcrumb={back}
        titleAction={data ? <RoleBadge role={data.role} /> : undefined}
      />
      {data?.cloneUrl ? <CloneAddress url={data.cloneUrl} /> : null}
      <Tabs
        id="repository"
        label={t("repositories.page.views")}
        tabs={tabs.map((value) => ({ value, label: t(`repositories.tab.${value}`) }))}
        value={tab}
        onChange={(value) =>
          void navigate({
            to: "/projects/$project/repositories/$name",
            params: { project, name },
            search: { tab: value, ...(search.ref ? { ref: search.ref } : {}) },
          })
        }
      />
      <div {...tabPanelProps("repository", tab)} className="min-w-0">
        {repository.isPending || !data ? (
          <PageLoading label={t("repositories.loading")} />
        ) : tab === "files" ? (
          <FilesView project={project} repository={data} search={search} />
        ) : tab === "commits" ? (
          <CommitsView project={project} repository={data} search={search} />
        ) : tab === "branches" ? (
          <BranchesView project={project} repository={data} />
        ) : tab === "pulls" ? (
          <PullsView project={project} repository={data} search={search} />
        ) : (
          <SettingsView project={project} repository={data} />
        )}
      </div>
    </section>
  );
}
