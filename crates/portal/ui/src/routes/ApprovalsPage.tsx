import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { isOwn } from "../api/approval";
import { api, queryKeys, unwrap } from "../api/client";
import { useAuth } from "../auth/AuthProvider";
import { AdminGrid } from "../components/grid/AdminGrid";
import type { DataColumn } from "../components/grid/AdminGrid";
import { ApprovalsHistory } from "./ApprovalsHistory";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import {
  Alert,
  Checkbox,
  EmptyState,
  ExternalLink,
  Icon,
  PageHeader,
  Select,
  Tabs,
  buttonClass,
  tabPanelProps,
  Term,
} from "../components/ui";
import { useUrlParam } from "../navigation/urlState";

export function ApprovalsPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const { identity } = useAuth();
  const [mine, setMine] = useState(false);
  const [phase, setPhase] = useState("");
  // Open changes wait for a decision; the history is what was decided (T-3292).
  // The tab is in the address, so back, reload and a sent link open it (T-3239).
  const [view, setView] = useUrlParam<"open" | "history">("tab", "open", ["open", "history"]);

  const list = useQuery({
    queryKey: queryKeys.changes(project),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/changes", {
          params: { path: { project } },
        }),
      ),
  });

  const header = (
    <PageHeader
        compact
      title={t("approvals.title")}
      description={t("approvals.lead")}
    />
  );

  const all = list.data?.items ?? [];
  // A fork's or a non-writer's merge request in the forge, which nobody can approve here; the
  // API names them to an organization administrator alone (T-3433).
  const outside = list.data?.outside ?? [];
  // The phases the list holds, in the order a change goes through them.
  const phases = [...new Set(all.map((proposal) => proposal.status.phase))];
  // Both filters narrow what the API already let this caller read; they decide nothing.
  const items = all.filter(
    (proposal) =>
      (!mine || isOwn(identity?.email, proposal)) &&
      (phase === "" || proposal.status.phase === phase),
  );
  const dateFormatter = new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  });

  type Proposal = (typeof all)[number];
  const summaryOf = (proposal: Proposal) => t(proposal.summary.key, proposal.summary.params as Record<string, unknown>);
  const columns: DataColumn<Proposal>[] = [
    {
      key: "summary",
      label: t("approvals.summary"),
      text: summaryOf,
      cell: (proposal) => (
        <>
          <Link
            data-row-link=""
            to="/projects/$project/approvals/$id"
            params={{ project, id: proposal.metadata.name }}
            className="focus-ring rounded-sm text-primary-soft-fg hover:underline"
          >
            {summaryOf(proposal)}
          </Link>
          <div className="mt-0.5 font-mono text-caption text-fg-subtle">{proposal.metadata.name}</div>
          {/* A change that waits on another merges after it (MF-48). */}
          {(proposal.waitsOn ?? [])
            .filter((awaited) => awaited.phase !== "Merged")
            .map((awaited) => (
              <div key={awaited.name} className="mt-0.5 text-caption text-fg-muted">
                {awaited.phase === "Rejected"
                  ? t("approvals.waitsOn.rejected", { change: awaited.name })
                  : t("approvals.waitsOn.pending", { change: awaited.name })}
              </div>
            ))}
          {/* A bundle is more than its headline, and the approval checks all of it. */}
          {(proposal.fileCount ?? 0) > 1 ? (
            <div className="mt-0.5 text-caption text-fg-muted">{t("approvals.fileCount", { count: proposal.fileCount })}</div>
          ) : null}
        </>
      ),
    },
    {
      key: "phase",
      label: t("approvals.phase"),
      text: (proposal) => proposal.status.phase,
      cell: (proposal) => <LifecycleBadge kind="phase" value={proposal.status.phase} />,
    },
    {
      key: "lane",
      label: t("approvals.lane"),
      heading: <Term name="lane">{t("approvals.lane")}</Term>,
      text: (proposal) => proposal.status.lane ?? "",
      cell: (proposal) => <LifecycleBadge kind="lane" value={proposal.status.lane} />,
    },
    { key: "author", label: t("approvals.author"), text: (proposal) => proposal.author.name },
    {
      key: "created",
      label: t("approvals.created"),
      kind: "date",
      text: (proposal) => proposal.createdAt,
      cell: (proposal) => <span className="whitespace-nowrap text-fg-muted">{dateFormatter.format(new Date(proposal.createdAt))}</span>,
    },
    {
      key: "actions",
      label: t("approvals.actions"),
      text: () => "",
      actions: true,
      align: "right",
      cell: (proposal) => (
        <Link to="/projects/$project/approvals/$id" params={{ project, id: proposal.metadata.name }} className={buttonClass("secondary", "sm")}>
          {t("approvals.view")}
          <Icon name="chevronRight" className="size-3.5" />
        </Link>
      ),
    },
  ];
  const filters =
    all.length > 0 ? (
      <div className="flex flex-wrap items-center gap-4">
        <Checkbox
          label={t("approvals.filterMine")}
          checked={mine}
          onChange={(event) => setMine(event.target.checked)}
        />
        <label className="flex items-center gap-2 text-body">
          {t("approvals.filterPhase")}
          <Select
            value={phase}
            onChange={(event) => setPhase(event.target.value)}
          >
            <option value="">{t("approvals.filterAll")}</option>
            {phases.map((value) => (
              <option key={value} value={value}>
                {t(`phase.${value.charAt(0).toLowerCase()}${value.slice(1)}`, {
                  defaultValue: value,
                })}
              </option>
            ))}
          </Select>
        </label>
      </div>
    ) : null;

  const tabs = (
    <Tabs
      id="approvals-view"
      label={t("approvals.title")}
      tabs={[
        { value: "open", label: t("approvals.tabs.open") },
        { value: "history", label: t("approvals.tabs.history") },
      ]}
      value={view}
      onChange={setView}
    />
  );

  if (view === "history") {
    return (
      <div className="flex flex-col gap-section">
        {header}
        {tabs}
        <div {...tabPanelProps("approvals-view", "history")}>
          <ApprovalsHistory project={project} />
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-section">
      {header}
      {tabs}
      <div
        {...tabPanelProps("approvals-view", "open")}
        className="flex flex-col gap-section"
      >
        {filters}
        {outside.length > 0 ? (
          <Alert
            tone="warning"
            title={t("approvals.outside.title", { count: outside.length })}
          >
            <ul className="mt-1 flex flex-col gap-1">
              {outside.map((request) => (
                <li key={request.number}>
                  <ExternalLink href={request.url}>
                    {t("approvals.outside.item", {
                      number: request.number,
                      author: request.author,
                    })}
                  </ExternalLink>
                  {": "}
                  {request.reason}
                </li>
              ))}
            </ul>
          </Alert>
        ) : null}
        <AdminGrid<Proposal>
          query={list}
          caption={t("approvals.title")}
          storageKey={`${project}:approvals`}
          stack
          rows={items}
          rowKey={(proposal) => proposal.metadata.name}
          columns={columns}
          empty={
            all.length === 0 ? (
              // Nothing proposed yet: the way to a first change is the assistant (T-1381).
              <EmptyState
                bare
                icon="approvals"
                title={t("approvals.empty")}
                description={t("approvals.emptyHint")}
                action={
                  <Link
                    to="/projects/$project/assistant"
                    params={{ project }}
                    className={buttonClass("primary", "md")}
                  >
                    {t("approvals.emptyAction")}
                  </Link>
                }
              />
            ) : (
              <EmptyState
                bare
                icon="approvals"
                title={t("approvals.noneMatch")}
              />
            )
          }
        />
      </div>
    </div>
  );
}
