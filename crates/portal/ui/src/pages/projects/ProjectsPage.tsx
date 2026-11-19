import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { useProjects, useProjectTitles } from "../../api/projects";
import { useMayOpenProject } from "../../api/permissions";
import { ORG_NAMESPACE } from "../../api/manifest";
import { NewProjectButton, NewProjectDialog } from "../../components/layout/NewProject";
import { Button, Card, EmptyState, Icon, PageFailed, PageHeader, PageLoading, buttonClass } from "../../components/ui";

/**
 * The Organization's projects the person reads (T-7380): each a way into its shell, New project
 * when the organization lets them open one, and with none a first-run page whose one action opens
 * the first project. Someone who may not open one reads who can and keeps the catalogue.
 */
export function ProjectsPage(): JSX.Element {
  const { t } = useTranslation();
  const projects = useProjects();
  const titleOf = useProjectTitles();
  const may = useMayOpenProject();
  const [writing, setWriting] = useState(false);
  const items = projects.data ?? [];
  const header = (
    <PageHeader
      title={t("projectsPage.title")}
      description={t("projectsPage.lead")}
      actions={items.length > 0 ? <NewProjectButton project={ORG_NAMESPACE} /> : undefined}
    />
  );
  if (projects.isError) {
    return (
      <div className="space-y-6">
        {header}
        <PageFailed error={projects.error} onRetry={() => void projects.refetch()} />
      </div>
    );
  }
  if (projects.isPending || (items.length === 0 && !may.known)) {
    return (
      <div className="space-y-6">
        {header}
        <PageLoading label={t("projects.loading")} />
      </div>
    );
  }
  if (items.length === 0) {
    return (
      <div className="space-y-6">
        {header}
        {may.allowed ? (
          <EmptyState
            heading={2}
            icon="plus"
            title={t("projectsPage.firstTitle")}
            description={t("projectsPage.firstLead")}
            action={
              <Button variant="primary" icon={<Icon name="plus" className="size-4" />} onClick={() => setWriting(true)}>
                {t("projectsPage.first")}
              </Button>
            }
          />
        ) : (
          <EmptyState
            heading={2}
            icon="lock"
            title={t("projectsPage.noneTitle")}
            description={[may.reason, t("projectsPage.noneLead")].filter(Boolean).join(" ")}
            action={
              <Link to="/catalogue" className={buttonClass("secondary", "md")}>
                {t("projectsPage.toCatalogue")}
              </Link>
            }
          />
        )}
        <NewProjectDialog open={writing} onOpenChange={setWriting} />
      </div>
    );
  }
  return (
    <div className="space-y-6">
      {header}
      <ul aria-label={t("projectsPage.title")} className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {items.map((name) => (
          <li key={name}>
            <Card flush className="h-full">
              <Link
                to="/projects/$project/home"
                params={{ project: name }}
                className="focus-ring flex h-full items-center gap-3 rounded-lg p-4 hover:bg-surface-subtle"
              >
                <span className="inline-flex size-9 shrink-0 items-center justify-center rounded-md bg-primary-soft text-primary-soft-fg">
                  <Icon name="folder" className="size-4" />
                </span>
                <span className="flex min-w-0 flex-col">
                  <span className="truncate font-medium text-fg">{titleOf(name)}</span>
                  <span className="truncate text-caption text-fg-muted">{name}</span>
                </span>
              </Link>
            </Card>
          </li>
        ))}
      </ul>
    </div>
  );
}
