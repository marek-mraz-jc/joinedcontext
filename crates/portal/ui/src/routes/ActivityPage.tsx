import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { ActivityFeed } from "../components/ActivityFeed";
import { ActivitySummary } from "../components/ActivitySummary";
import { PageHeader } from "../components/ui";

/** What is happening in this project, filtered and tailed (UI-31, OPS-48). */
export function ActivityPage({
  project,
  kind,
}: {
  project: string;
  /** The kind filter of the page's address, `?kind=mcp.tool`. */
  kind?: string;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-section">
      <PageHeader title={t("activity.title")} description={t("activity.lead")} />
      <ActivitySummary project={project} />
      <ActivityFeed project={project} initialKind={kind} />
    </div>
  );
}
