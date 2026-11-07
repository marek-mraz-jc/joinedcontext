import { Suspense, lazy, useEffect, useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, localized, refName } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { proposeChecked } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { useFormRoute } from "../../components/forms/FormRoute";
import { ResourceRowActions } from "../../components/ResourceRowActions";
import type { RowAction } from "../../components/ui/RowActions";
import { usePermissions } from "../../api/permissions";
import { appSchema, appUiSchema } from "../../schemas/kinds";
import { fromAppEnvelope, toAppEnvelope } from "./appForm";
import type { AppForm } from "./appForm";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { LifecycleBadge } from "../../components/status/LifecycleBadge";
import { AppCheckChip, useAppChecks } from "./AppCheckChip";
import type { AppCheck } from "./AppCheckChip";
import { mayPublish, publishChecklist } from "./publishChecklist";
import { useBranding } from "../../branding";
import { Icon } from "../../components/ui/icons";
import { requestOpen } from "../../assistant/state";
import { AppBuildState, runState, useAppBuild, useRebuild } from "./AppBuildPanel";
import type { components } from "../../api/schema";
import { AgentRunPage } from "./AgentRunPage";
import { appDisplayName, useEndpointTitles } from "./appTitle";
import { runInUrl, setRunInUrl } from "./useAgentRun";
import {
  Alert,
  Badge,
  Button,
  buttonClass,
  Dialog,
  EmptyState,
  InlineError,
  PageFailed,
  PageHeader,
  PageLoading,
  recordCard,
  safeHref,
} from "../../components/ui";
import { RECORD_LINK_STYLE, RecordLink } from "../../components/RecordLink";
import { RenameShapesNotice } from "./RenameShapesNotice";
import { AppTemplates } from "./AppTemplates";

// Loaded on first open: the dialog is not needed to draw the page (bundle cap).
const ShareDialog = lazy(() => import("./ShareDialog").then((module) => ({ default: module.ShareDialog })));

type WorkflowRun = components["schemas"]["WorkflowRun"];
type AppBuild = components["schemas"]["AppBuild"];

interface DataNeed {
  contextSpaceRef?: string | { name?: string };
  types?: string[];
  attrs?: string[];
}

export interface AppSpec {
  /** `static` or `full-stack`; `spec.kind` in the manifest (AP-01). */
  kind?: string;
  visibility?: string;
  /** `draft` → `preview` → `published` → `retired` (AP-18). */
  lifecycle?: string;
  /** Whether the Portal may frame the app at all (AP-12). */
  embeddable?: boolean;
  dataNeeds?: DataNeed[];
}

export function appSpec(app: Manifest): AppSpec {
  return app.spec as AppSpec;
}

/**
 * Whether the static host has something to serve under the app's name: a build the lane
 * published, or the bundle the Portal image ships (AP-86, AP-87). A published App with neither
 * answers 404, so the catalog offers no Open on it.
 */
export function isServed(app: Manifest): boolean {
  return Boolean(app.status?.build) || app.metadata.annotations?.["joinedcontext.com/shipped-with"] === "portal";
}

/**
 * Why a published App cannot be opened yet, in the catalog's words, or `undefined` when a build
 * is served (AP-86): the catalog card, the App's page and the in-Portal page say the same thing.
 */
export function openBlockedReason(app: Manifest, run: WorkflowRun | null, t: TFunction): string | undefined {
  const lifecycle = appSpec(app).lifecycle ?? "draft";
  if (lifecycle !== "published") {
    return t(`apps.openDisabled.${lifecycle === "preview" || lifecycle === "retired" ? lifecycle : "draft"}`);
  }
  // Its own host answers once its certificate is issued (AP-133): until then there is nothing
  // to open, and the reconciler says so on the App.
  const host = (app.status?.conditions ?? []).find((condition) => condition.type === "Ready");
  if (host?.status === "False" && host.reason === "CertificatePending") return t("apps.openDisabled.certificate");
  if (host?.status === "False" && host.reason === "HostRefused") return t("apps.openDisabled.host");
  if (isServed(app)) return undefined;
  if (run && runState(run) === "building") return t("apps.openDisabled.building");
  if (run && runState(run) === "failed") return t("apps.openDisabled.failed");
  return t("apps.openDisabled.notBuilt");
}

/** Who an app is visible to, in the reader's words rather than the manifest's enum (T-3214). */
export function visibilityLabel(visibility: string, t: TFunction): string {
  return t(`apps.audience.${visibility}`, { defaultValue: visibility });
}

export function draftState(
  status: string,
): "building" | "needsYou" | "failed" | "readyToPublish" | "waitingApproval" | null {
  switch (status) {
    case "queued":
    case "starting":
    case "building":
    case "testing":
      return "building";
    // A preview is what a person publishes (AP-46); once proposed, the change waits for an
    // approver, which is not the same thing and not the person's to do (T-2772).
    case "previewing":
      return "readyToPublish";
    case "interviewing":
      return "needsYou";
    case "awaiting_approval":
    case "awaitingApproval":
      return "waitingApproval";
    case "failed":
    case "cancelled":
    case "expired":
      return "failed";
    case "published":
      return null;
    default:
      return null;
  }
}

interface CatalogRun {
  id: string;
  appName: string;
  /** What the application is called; absent from older runs, which show the name as words. */
  title?: string;
  endpointName?: string;
  status: string;
  prompt?: string;
  error?: string;
  createdAt: string;
}

/** The space a data need names, whichever of the two `Ref` spellings the manifest used. */
function spaceOf(need: DataNeed): string {
  return refName(need.contextSpaceRef);
}

/**
 * The preview URL of Architecture/16 §4: the app under the platform host, with the commit it
 * was built from, so a reviewer is never looking at a cached older build.
 */
export function previewUrl(app: Manifest): string {
  const revision = app.status?.observedRevision;
  const at = revision ? `?preview=${encodeURIComponent(revision)}` : "";
  return `/apps/${encodeURIComponent(app.metadata.name)}/${at}`;
}

/**
 * The sandboxed preview of AP-19.
 *
 * `sandbox` deliberately does NOT carry `allow-same-origin`. A static app is served from
 * `https://{host}/apps/{name}/` (AP-14), which is the Portal's own origin, and the pair
 * `allow-scripts allow-same-origin` on a same-origin frame is not a sandbox at all: the
 * framed document could reach into the Portal, read the deliberately readable `__Host-jc_csrf`
 * cookie and issue writes as the signed-in reviewer, whose session cookie the browser would
 * attach. With `allow-scripts` alone the frame runs its code in an opaque origin, which is
 * what makes previewing somebody else's generated app safe to do while signed in.
 *
 * The server half is already in place: the static host sends `frame-ancestors 'none'` and
 * `X-Frame-Options: DENY` for an app whose manifest does not say `embeddable` (AP-12), so an
 * app that may not be framed shows its reason here rather than an empty rectangle.
 */
export function AppPreview({ app, onClose }: { app: Manifest; onClose: () => void }): JSX.Element {
  const { t, i18n } = useTranslation();
  const spec = appSpec(app);
  const title = localized(app.metadata.title, i18n.language, app.metadata.name);

  return (
    <div className="space-y-3">
      <Button onClick={onClose}>
        {t("apps.back")}
      </Button>

      <PageHeader title={t("apps.preview.title", { name: title })} description={t("apps.preview.sandboxSpace")} />

      {spec.embeddable ? (
        <iframe
          title={t("apps.preview.title", { name: title })}
          src={previewUrl(app)}
          sandbox="allow-scripts"
          referrerPolicy="no-referrer"
          className="h-128 w-full rounded border border-border bg-surface"
        />
      ) : (
        <InlineError>
          {t("apps.preview.notEmbeddable")}
        </InlineError>
      )}
    </div>
  );
}

/**
 * The apps catalog (AP-18…AP-20, AP-24).
 *
 * Publishing is a repository change like every other write in the Portal: the manifest's
 * lifecycle flips to `published` and the answer is a merge request, never a live switch. The
 * lane the change lands in follows the app's `visibility`, which is why the confirmation says
 * what is about to become reachable and to whom.
 */
export function AppsCatalog({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [previewing, setPreviewing] = useState<Manifest | null>(null);
  // A run stays where it is: the address names it, so a closed tab reopens the same build.
  const [runId, setRunId] = useState<string | null>(() => runInUrl());
  // The lifecycle move waiting for its confirmation: publish a preview, or retire a published app.
  const [confirming, setConfirming] = useState<{ app: Manifest; lifecycle: Lifecycle } | null>(null);
  const [sharing, setSharing] = useState<Manifest | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);

  // `/apps/new` is what "New app" does: the assistant's builder over this list (T-2750). A role
  // that may not propose an App keeps the address's own "no create form here" instead.
  const formRoute = useFormRoute();
  const permissions = usePermissions(project);
  const asksNew = formRoute?.form?.mode === "new";
  const mayBuild = permissions.can("App", "propose");
  const closeRoute = formRoute?.close;
  // Once per arrival at the address: the effect runs again while the way back to the list is
  // still under way, and asking twice would navigate against itself.
  const handedToBuilder = useRef(false);
  useEffect(() => {
    if (!asksNew) {
      handedToBuilder.current = false;
      return;
    }
    if (handedToBuilder.current || permissions.isLoading || !mayBuild || !closeRoute) {
      return;
    }
    handedToBuilder.current = true;
    requestOpen("build");
    closeRoute();
  }, [asksNew, permissions.isLoading, mayBuild, closeRoute]);

  const list = useQuery({
    queryKey: queryKeys.list(project, "apps"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "apps" } },
        }),
      ),
  });

  const runs = useQuery({
    queryKey: [...queryKeys.list(project, "apps"), "runs"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/agent-runs", {
          params: { path: { project }, query: { limit: 20 } },
        }),
      ),
    refetchInterval: runId === null ? 15000 : false,
  });
  const endpointTitles = useEndpointTitles(project);
  const appChecks = useAppChecks(project);

  const publish = useMutation({
    mutationFn: async ({ app, lifecycle }: { app: Manifest; lifecycle: Lifecycle }) => {
      setError(null);
      // `status` is the API's own computation (MF-04); a write never sends it back.
      const body = {
        apiVersion: app.apiVersion,
        kind: app.kind,
        metadata: app.metadata,
        spec: { ...appSpec(app), lifecycle },
      };
      // Checked first, then written: the verdict gate refuses a manifest nothing checked, so a
      // click that only wrote was refused with the gate's own sentence (PF-57, T-2264).
      return proposeChecked(project, "apps", body, false);
    },
    onSuccess: (result) => {
      setConfirming(null);
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "apps") });
    },
    onError: (err) => {
      setConfirming(null);
      setError(
        err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic"),
      );
    },
  });

  if (list.isPending) {
    return (
      <div className="space-y-4">
        <PageHeader compact title={t("apps.title")} description={t("apps.subtitle")} />
        <PageLoading label={t("app.loading")} />
      </div>
    );
  }

  // The page keeps its heading and says what failed in the box every list uses (T-4165).
  if (list.isError) {
    return (
      <div className="space-y-4">
        <PageHeader compact title={t("apps.title")} description={t("apps.subtitle")} />
        <PageFailed
          error={list.error}
          onRetry={() => {
            void list.refetch();
          }}
        />
      </div>
    );
  }

  if (previewing) {
    return (
      <AppPreview
        app={previewing}
        onClose={() => {
          setPreviewing(null);
        }}
      />
    );
  }

  if (runId !== null) {
    return (
      <AgentRunPage
        project={project}
        runId={runId}
        onClose={() => {
          setRunInUrl(null);
          setRunId(null);
          void runs.refetch();
        }}
      />
    );
  }

  const apps = asManifests(list.data?.items ?? []);
  const publishedNames = new Set(apps.map((app) => app.metadata.name));

  const runItems = (runs.data?.items ?? []) as unknown as CatalogRun[];
  const draftRuns: CatalogRun[] = [];
  const seenDraftApps = new Set<string>();
  for (const r of runItems) {
    if (r.appName && !publishedNames.has(r.appName) && !seenDraftApps.has(r.appName)) {
      seenDraftApps.add(r.appName);
      const state = draftState(r.status);
      // An expired, cancelled or failed build is history, not an application: it stays in the
      // assistant's past work, not in this grid.
      if (state !== null && state !== "failed") {
        draftRuns.push(r);
      }
    }
  }

  // The builds whose change waits for an approver, said once above the grid (T-2772): a run
  // that proposed used to expire twenty minutes later with nobody told.
  const waiting = draftRuns.filter((draft) => draftState(draft.status) === "waitingApproval").length;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <PageHeader compact
          title={t("apps.title")}
          description={t("apps.subtitle")}
        />
        <PermissionGuard project={project} kind="App" verb="propose">
          <Button
            onClick={() => {
              requestOpen("build");
            }}
          >
            {t("apps.newAction")}
          </Button>
        </PermissionGuard>
      </div>

      {/* Start from a template: only the endpoint to read is asked (T-3263). */}
      <AppTemplates project={project} mayBuild={mayBuild} />
      {change && <ChangeNotice change={change} project={project} />}
      <RenameShapesNotice project={project} apps={apps} proposed={change !== null} onProposed={setChange} />
      {waiting > 0 ? (
        <Alert
          tone="info"
          actions={
            <Link to="/projects/$project/approvals" params={{ project }} className={buttonClass("secondary", "sm")}>
              {t("apps.drafts.openApprovals")}
            </Link>
          }
        >
          {t("apps.drafts.waiting", { count: waiting })}
        </Alert>
      ) : null}
      {error && (
        <InlineError>
          {error}
        </InlineError>
      )}
      <div aria-live="polite">{notice ? <p className="text-sm">{notice}</p> : null}</div>

      {apps.length === 0 && draftRuns.length === 0 && (
        <EmptyState
          title={t("apps.emptyTitle")}
          description={t("apps.empty")}
          icon="apps"
          action={
            <PermissionGuard project={project} kind="App" verb="propose">
              <Button onClick={() => requestOpen("build")}>{t("apps.newAction")}</Button>
            </PermissionGuard>
          }
        />
      )}

      <ul className="grid grid-cols-[repeat(auto-fill,minmax(14rem,1fr))] gap-3">
        {apps.map((app) => {
          const spec = appSpec(app);
          const title =
            localized(app.metadata.title, i18n.language, "") || appDisplayName({ appName: app.metadata.name });
          const needs = spec.dataNeeds ?? [];
          return (
            <li
              key={app.metadata.name}
              title={localized(app.metadata.description, i18n.language, "") || undefined}
              onClick={recordCard.onClick}
              onAuxClick={recordCard.onAuxClick}
              className={`flex flex-col items-center gap-2 rounded-xl border border-border bg-surface p-4 text-center hover:bg-surface-subtle ${recordCard.className}`}
            >
              <AppIcon />
              <h2 className="line-clamp-2 text-sm font-semibold">
                <AppCardLink project={project} app={app}>
                  {title}
                </AppCardLink>
              </h2>
              <LifecycleBadge kind="appLifecycle" value={spec.lifecycle ?? "draft"} />
              {spec.lifecycle === "published" ? <AppCheckChip check={appChecks.get(app.metadata.name)} /> : null}
              {spec.visibility ? (
                <p className="text-xs text-fg-muted">
                  {t("apps.visibility", { visibility: visibilityLabel(spec.visibility, t) })}
                </p>
              ) : null}
              {needs.length > 0 && (
                <p className="line-clamp-2 text-xs text-fg-muted">
                  {t("apps.dataNeeds", {
                    spaces: [...new Set(needs.map(spaceOf).filter(Boolean))].join(", "),
                    types: [...new Set(needs.flatMap((need) => need.types ?? []))].join(", "),
                  })}
                </p>
              )}

              {spec.lifecycle === "published" && (
                <AppBuildState
                  project={project}
                  name={app.metadata.name}
                  served={app.status?.build?.commit ?? null}
                  shipped={app.metadata.annotations?.["joinedcontext.com/shipped-with"] === "portal"}
                />
              )}

              <AppCardActions
                project={project}
                app={app}
                title={title}
                onPreview={() => {
                  setPreviewing(app);
                }}
                onLifecycle={(lifecycle) => {
                  setConfirming({ app, lifecycle });
                }}
                onShare={() => {
                  setSharing(app);
                }}
                onRebuild={(outcome) => {
                  setError(outcome.error ?? null);
                  setNotice(outcome.error ? null : t("apps.build.started"));
                }}
              />
            </li>
          );
        })}
        {draftRuns.map((draft) => {
          const state = draftState(draft.status);
          if (!state) return null;
          return (
            <li
              key={draft.id}
              title={draft.prompt || undefined}
              className="flex flex-col items-center gap-2 rounded-xl border border-border bg-surface p-4 text-center hover:bg-surface-subtle"
            >
              <AppIcon />
              <h2 className="line-clamp-2 text-sm font-semibold">
                {appDisplayName({
                  title: draft.title,
                  appName: draft.appName,
                  endpointTitle: draft.endpointName ? endpointTitles.get(draft.endpointName) : undefined,
                })}
              </h2>
              <span className="rounded bg-surface-subtle px-2 py-0.5 text-xs font-medium text-fg-muted">
                {t(`apps.drafts.state.${state}`)}
              </span>
              <div className="mt-auto flex flex-wrap justify-center gap-2">
                <Link
                  to="/projects/$project/$plural/$name"
                  params={{ plural: "apps", project, name: draft.appName }}
                  className={buttonClass("secondary", "sm")}
                >
                  {t("apps.drafts.open")}
                </Link>
              </div>
            </li>
          );
        })}
      </ul>

      {sharing && (
        <Suspense fallback={null}>
          <ShareDialog
            project={project}
            app={sharing}
            onClose={() => {
              setSharing(null);
            }}
          />
        </Suspense>
      )}
      {confirming && (
        <LifecycleDialog
          project={project}
          check={appChecks.get(confirming.app.metadata.name)}
          app={confirming.app}
          lifecycle={confirming.lifecycle}
          pending={publish.isPending}
          onCancel={() => {
            setConfirming(null);
          }}
          onConfirm={() => {
            publish.mutate(confirming);
          }}
        />
      )}
    </div>
  );
}

/**
 * The card's link, which a click anywhere on the card follows (T-2875): an App something serves
 * opens itself, framed under the Portal's header (T-3038, AP-122); any other App opens its page,
 * which says why there is nothing to open yet. Its details stay one item of the card's menu.
 */
function AppCardLink({ project, app, children }: { project: string; app: Manifest; children: string }): JSX.Element {
  const { t } = useTranslation();
  const name = app.metadata.name;
  const build = useAppBuild(project, name);
  return openBlockedReason(app, build.data?.run ?? null, t) === undefined ? (
    <Link
      data-row-link=""
      to="/projects/$project/$plural/$name/open"
      params={{ project, plural: "apps", name }}
      className={RECORD_LINK_STYLE}
    >
      {children}
    </Link>
  ) : (
    <RecordLink project={project} plural="apps" name={name}>
      {children}
    </RecordLink>
  );
}

/**
 * An App's links into the forge, in the order its card menu lists them (AP-103, T-3039): its
 * whole source, the project's configuration, its manifest's history, the newest build run and
 * the package. Each one is there only when the platform has the address for this person.
 */
export function forgeLinks(app: Manifest, build: AppBuild | undefined, t: TFunction): RowAction[] {
  const links: [string, string, string | null | undefined][] = [
    ["sourceCode", t("apps.sourceCode"), build?.repositoryUrl],
    ["configuration", t("apps.projectConfiguration"), build?.configurationUrl],
    ["source", t("apps.history"), app.status?.sourceUrl],
    ["run", t("apps.latestRun"), build?.run?.url],
    ["package", t("apps.package"), build?.packageUrl],
  ];
  return links.flatMap(([key, label, url]) => {
    const href = safeHref(url);
    return href ? [{ key, label, href }] : [];
  });
}

/** The tile's icon: every application gets the same mark until a manifest carries its own. */
function AppIcon(): JSX.Element {
  return (
    <span className="flex size-14 items-center justify-center rounded-2xl bg-primary-soft text-primary-soft-fg">
      <Icon name="apps" className="size-7" />
    </span>
  );
}

/**
 * The lifecycle confirmation (AP-20). It says what the move does rather than asking "are you
 * sure": publishing takes the app out of its sandbox space, binds it to the real one and makes it
 * reachable by the audience its `visibility` names; retiring takes it away from that audience.
 */
function LifecycleDialog({
  project,
  check,
  app,
  lifecycle,
  pending,
  onCancel,
  onConfirm,
}: {
  project: string;
  check: AppCheck | undefined;
  app: Manifest;
  lifecycle: Lifecycle;
  pending: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const spec = appSpec(app);
  const title = localized(app.metadata.title, i18n.language, app.metadata.name);
  const copy = lifecycle === "published" ? "publish" : "retire";
  const branding = useBranding();
  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    enabled: lifecycle === "published",
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural: "endpoints" } } })),
  });
  // What the app lacks before it is published (AP-140): warnings may stay, a privacy problem not.
  const items =
    lifecycle === "published"
      ? publishChecklist(app, {
          licenceDefault: branding.licenseDefault,
          contactEmail: branding.contactEmail,
          check,
          endpoints: asManifests(endpoints.data?.items ?? []),
        })
      : [];
  const blocked = !mayPublish(items);

  // The shared dialog (T-3281): it was a panel that called itself modal, so a screen reader was
  // told the page behind it was gone while Tab still walked through it.
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
      title={t(`apps.${copy}.title`, { name: title })}
      description={t(`apps.${copy}.body`, { visibility: visibilityLabel(spec.visibility ?? "project", t) })}
      closeLabel={t("app.close")}
      footer={
        <>
          <Button onClick={onCancel}>{t(`apps.${copy}.cancel`)}</Button>
          <Button
            variant={lifecycle === "retired" ? "danger" : "primary"}
            disabled={pending || blocked || (lifecycle === "published" && endpoints.isPending)}
            disabledReason={blocked ? t("apps.checklist.blocked") : undefined}
            onClick={onConfirm}
          >
            {t(`apps.${copy}.confirm`)}
          </Button>
        </>
      }
    >
      <p className="text-sm text-fg-muted">{t(`apps.${copy}.hint`)}</p>
      {items.length > 0 ? (
        <section aria-labelledby="publish-checklist" className="mt-3">
          <h3 id="publish-checklist" className="text-body font-semibold">
            {t("apps.checklist.heading")}
          </h3>
          {endpoints.isPending ? <p className="text-caption text-fg-muted">{t("apps.checklist.reading")}</p> : null}
          <ul className="mt-1 flex flex-col gap-1">
            {items.map((item, index) => (
              <li key={`${item.key}-${index}`} className="flex flex-wrap items-baseline gap-2 text-sm">
                <Badge tone={item.state === "ok" ? "success" : item.state === "warning" ? "warning" : "danger"}>
                  {t(`apps.checklist.state.${item.state}`)}
                </Badge>
                <span>
                  {t(`apps.checklist.${item.key}.${item.reason ?? item.state}`, { detail: item.detail ?? "" })}
                </span>
                {item.state !== "ok" ? (
                  <Link
                    to={item.key === "checks" ? "/projects/$project/$plural/$name" : "/projects/$project/$plural/$name/edit"}
                    params={{ project, plural: "apps", name: app.metadata.name }}
                    className={RECORD_LINK_STYLE}
                  >
                    {t(`apps.checklist.fix.${item.key}`)}
                  </Link>
                ) : null}
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </Dialog>
  );
}

type Lifecycle = "published" | "retired";

/**
 * The footer of an application's card (T-2618, T-3038, UI-26, UI-44): why there is nothing to
 * open when there is not, and one ⋯ menu that holds everything else; opening is the card's own
 * click. The menu lists every action in three blocks (the lifecycle, the App's page and the
 * forge, the manifest's own four), and an action that does not apply stays listed, disabled,
 * with its sentence.
 */
function AppCardActions({
  project,
  app,
  title,
  onPreview,
  onLifecycle,
  onShare,
  onRebuild,
}: {
  project: string;
  app: Manifest;
  title: string;
  onPreview: () => void;
  onLifecycle: (lifecycle: Lifecycle) => void;
  onShare: () => void;
  onRebuild: (outcome: { error?: string }) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const permissions = usePermissions(project);
  const name = app.metadata.name;
  const lifecycle = appSpec(app).lifecycle ?? "draft";
  const published = lifecycle === "published";
  const build = useAppBuild(project, name);
  const rebuild = useRebuild(project, name);
  const run = build.data?.run ?? null;
  const running = run !== null && runState(run) === "building";
  const mayPropose = permissions.can("App", "propose");
  const denied = t("permissions.denied", { verb: "propose", kind: "App" });

  const openReason = openBlockedReason(app, run, t);

  const rebuildReason = !published
    ? t("apps.rebuildOnlyPublished")
    : running
      ? t("apps.rebuildRunning")
      : build.data && !build.data.rebuild.allowed
        ? (build.data.rebuild.reason ?? t("apps.build.notOnForge"))
        : !build.data
          ? t("apps.build.notOnForge")
          : undefined;

  const lifecycleActions: RowAction[] = [
    {
      key: "preview",
      label: t("apps.previewAction"),
      onSelect: onPreview,
      disabledReason: lifecycle === "preview" ? undefined : t("apps.previewOnlyPreview"),
    },
    {
      key: "publish",
      label: t("apps.publishAction"),
      onSelect: () => onLifecycle("published"),
      disabledReason: lifecycle !== "preview" ? t("apps.publishOnlyPreview") : mayPropose ? undefined : denied,
    },
    {
      key: "rebuild",
      label: t("apps.rebuildAction"),
      onSelect: () =>
        rebuild.mutate(undefined, {
          onSuccess: () => onRebuild({}),
          onError: (err) =>
            onRebuild({
              error: err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic"),
            }),
        }),
      disabledReason: rebuildReason,
    },
    {
      key: "retire",
      label: t("apps.retireAction"),
      tone: "danger",
      onSelect: () => onLifecycle("retired"),
      disabledReason: !published ? t("apps.retireOnlyPublished") : mayPropose ? undefined : denied,
    },
  ];
  // AP-24: every iteration with the agent is a commit, and the prompt history lives with the
  // source; the forge shows it, its runs and its packages (AP-103). The App's whole source and
  // the project's configuration come as the API reads them for this person (T-3039): a link
  // the forge would refuse them is not offered. Listed only where they exist.
  const forgeActions: RowAction[] = forgeLinks(app, build.data, t);
  // The App's page (settings, versions, build) now that a click on the card opens the App (T-3038).
  const details: RowAction = {
    key: "details",
    label: t("apps.openPage.details"),
    onSelect: () => {
      void navigate({ to: "/projects/$project/$plural/$name", params: { project, plural: "apps", name } });
    },
  };
  // Who opens the App, proposed as a Change (AP-173); a retired App reaches nobody.
  const share: RowAction = {
    key: "share",
    label: t("apps.share.action"),
    onSelect: onShare,
    disabledReason: lifecycle === "retired" ? t("apps.share.retired") : mayPropose ? undefined : denied,
  };
  const extra = [
    ...lifecycleActions.slice(0, -1),
    { ...lifecycleActions[lifecycleActions.length - 1], separatorAfter: true },
    share,
    details,
    ...forgeActions,
  ];

  return (
    <div className="mt-auto flex flex-col items-center gap-2">
      {/* Open is the card itself (T-3038); a card with nothing to open says why, in words. A
          retired app is gone, and its badge says so. */}
      {openReason && lifecycle !== "retired" ? <p className="text-xs text-fg-muted">{openReason}</p> : null}
      <ResourceRowActions
        project={project}
        target={{ project, kind: "App", plural: "apps", name, label: title }}
        extra={extra}
        // The kind's own form, not the manifest as text (T-2343). Publishing and retiring stay
        // the menu's own items, with their confirmation: the form keeps the stored lifecycle.
        form={{
          schema: appSchema(t),
          uiSchema: appUiSchema,
          fromManifest: (manifest) => fromAppEnvelope(manifest) as unknown as Record<string, unknown>,
          toManifest: (edited, stored) => toAppEnvelope(project, edited as unknown as AppForm, stored),
        }}
      />
    </div>
  );
}
