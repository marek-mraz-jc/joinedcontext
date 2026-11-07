import { Suspense, lazy, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { Button, PageHeader, PageLoading } from "../../components/ui";
import { ResourcePageFailed } from "../../components/ui/PageState";
import { AgentRunPage } from "./AgentRunPage";
import { AppBuildPanel } from "./AppBuildPanel";
import { AppCheckChip, useAppChecks } from "./AppCheckChip";
import { AppGenerator } from "./AppGenerator";
import { AppJobRuns } from "./AppJobRuns";
import { AppServices } from "./AppServices";
import { OpenAppButton, useApp } from "./AppOpenPage";
import { appSpec, visibilityLabel } from "./AppsCatalog";
import { localized } from "../../api/manifest";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { appDisplayName } from "./appTitle";
import { RolesAndMembers } from "./RolesAndMembers";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import type { AgentRun } from "./useAgentRun";

// Loaded on first open: the dialog is not needed to draw the page (bundle cap).
const ShareDialog = lazy(() => import("./ShareDialog").then((module) => ({ default: module.ShareDialog })));
const DashboardEditor = lazy(() => import("./DashboardEditor").then((module) => ({ default: module.DashboardEditor })));

/**
 * An application and its runs (AP-68, AP-69, T-0559).
 *
 * Loads the application's runs, showing the newest with `AgentRunPage` which replays the
 * conversation and attaches the live stream. If no run exists yet, shows `AppGenerator`
 * prefilled with the application name.
 *
 * Each of the four states is a whole page rather than one line of text: the application is named
 * in the heading and in the tab before its runs are known, waiting draws the shape of what is
 * coming, and a failure carries the API's own sentence, a retry and the way back — a person who
 * opened an application by its address used to get "Loading..." on blank white, then a red line
 * with no heading and nothing to press (UI-15, UI-16).
 */
export function AppPage({ project, name }: { project: string; name: string }): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: ["projects", project, "agent-runs", name],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/agent-runs", {
          params: { path: { project }, query: { app: name } },
        }),
      ),
  });

  const back = () => {
    void navigate({ to: "/projects/$project/$plural", params: { project, plural: "apps" } });
  };

  // The name read as words until a run or an endpoint says what the application is called: the
  // heading and the tab say which application this is from the first paint (AP-69).
  const title = appDisplayName({ appName: name });
  const check = useAppChecks(project).get(name);

  if (isPending) {
    return (
      <div className="space-y-4">
        <PageHeader
          title={title}
          description={t("apps.subtitle")}
          actions={<Button onClick={back}>{t("apps.back")}</Button>}
        />
        <PageLoading label={t("app.loading")} />
      </div>
    );
  }

  if (isError) {
    return (
      <ResourcePageFailed
        title={title}
        description={t("apps.subtitle")}
        error={error}
        onRetry={() => {
          void refetch();
        }}
        back={<Button onClick={back}>{t("apps.back")}</Button>}
      />
    );
  }

  const items = (data?.items ?? []) as unknown as AgentRun[];
  const newest = items[0];

  // Where the published application is built and who holds its roles, above its runs (AP-103,
  // AP-99).
  if (newest) {
    return (
      <div className="space-y-3">
        {/* `empty:` because the button is not there before the App is read or once it is retired,
            and an empty row would still push the page down by the column's gap (T-2850). */}
        <div className="flex items-center justify-end gap-3 empty:hidden">
          <AppCheckChip check={check} />
          <OpenAppButton project={project} name={name} />
        </div>
        <AppBuildPanel project={project} name={name} />
        <AppJobRuns project={project} name={name} />
        <RolesAndMembers project={project} name={name} />
        <AgentRunPage project={project} runId={newest.id} onClose={back} />
        {/* After the run's page, whose heading is the page's H1 (UI-16). */}
        <AppServices project={project} name={name} />
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <AppHeading project={project} name={name} />
      <div className="flex flex-wrap justify-between gap-2">
        <Button onClick={back}>{t("apps.back")}</Button>
        <span className="flex items-center gap-3">
          <AppCheckChip check={check} />
          <OpenAppButton project={project} name={name} />
        </span>
      </div>
      <AppBuildPanel project={project} name={name} />
      <AppJobRuns project={project} name={name} />
      <RolesAndMembers project={project} name={name} />
      <DashboardFor project={project} name={name} />
      <GeneratorFor project={project} name={name} />
      <AppServices project={project} name={name} />
    </div>
  );
}

/**
 * The App's title, state, audience and description above everything else (T-4264, T-4160 #35):
 * a published App with no run of its own used to open on "Generate your own app". Nothing for a
 * name no App holds yet, whose page is the generator.
 */
function AppHeading({ project, name }: { project: string; name: string }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const app = useApp(project, name);
  const [sharing, setSharing] = useState(false);
  if (app.data?.kind !== "App") return null;
  const spec = appSpec(app.data);
  return (
    <div className="space-y-2">
      <PageHeader
        title={localized(app.data.metadata.title, i18n.language, "") || appDisplayName({ appName: name })}
        description={localized(app.data.metadata.description, i18n.language, "") || t("apps.pagePurpose")}
      />
      <p className="flex flex-wrap items-center gap-2 text-caption text-fg-muted">
        <LifecycleBadge kind="appLifecycle" value={spec.lifecycle ?? "draft"} />
        {spec.visibility ? t("apps.visibility", { visibility: visibilityLabel(spec.visibility, t) }) : null}
        {spec.lifecycle === "retired" ? null : (
          <PermissionGuard project={project} kind="App" verb="propose">
            <Button size="sm" onClick={() => setSharing(true)}>
              {t("apps.share.action")}
            </Button>
          </PermissionGuard>
        )}
      </p>
      {sharing ? (
        <Suspense fallback={null}>
          <ShareDialog project={project} app={app.data} onClose={() => setSharing(false)} />
        </Suspense>
      ) : null}
    </div>
  );
}

/** The generator, open for a name no App holds yet and behind its action for one that exists. */
function GeneratorFor({ project, name }: { project: string; name: string }): JSX.Element {
  const { t } = useTranslation();
  const app = useApp(project, name);
  if (app.data?.kind !== "App") return <AppGenerator project={project} initialName={name} />;
  return (
    <details className="group rounded-md border border-border p-3">
      <summary className="focus-ring cursor-pointer rounded-sm font-medium text-primary-soft-fg">
        {t("apps.editWithAssistant")}
      </summary>
      <div className="mt-3">
        <AppGenerator project={project} initialName={name} embedded />
      </div>
    </details>
  );
}

/**
 * The dashboard editor behind its action (T-6402): the editor reads the App's file only when a
 * person opens it, and a name no App holds yet has none.
 */
export function DashboardFor({ project, name }: { project: string; name: string }): JSX.Element | null {
  const { t } = useTranslation();
  const app = useApp(project, name);
  const [open, setOpen] = useState(false);
  if (app.data?.kind !== "App") return null;
  return (
    <details className="group rounded-md border border-border p-3" onToggle={(e) => setOpen(e.currentTarget.open)}>
      <summary className="focus-ring cursor-pointer rounded-sm font-medium text-primary-soft-fg">{t("apps.dashboard.section")}</summary>
      {open ? (
        <div className="mt-3">
          <Suspense fallback={<PageLoading label={t("app.loading")} />}>
            <DashboardEditor project={project} name={name} />
          </Suspense>
        </div>
      ) : null}
    </details>
  );
}
