import { Suspense, lazy, useEffect } from "react";
import { keepCopy, keepOrganization, ORG_NAME } from "./components/layout/keepCopy";
import {
  createRootRouteWithContext,
  createRoute,
  createRouter,
  Navigate,
  notFound,
  Outlet,
  redirect,
  useChildMatches,
  useRouteContext,
  useNavigate,
} from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { lastProject, usePreferredProject, useProjects } from "./api/projects";
import { BrandMark } from "./components/layout/BrandMark";
import { Shell } from "./components/layout/LazyShell";
import { PublicFrame } from "./components/layout/PublicFrame";
import { MemberFrame } from "./components/layout/MemberFrame";
import { isHiddenSection } from "./components/layout/navigation";
import { useHiddenSections } from "./branding";
import { EmptyState, PageFailed, PageLoading } from "./components/ui";
import { ChunkFailed, ErrorPage, errorReference, isChunkLoadError } from "./components/ErrorBoundary";
import { NotFoundState } from "./components/NotFoundState";
import { LoginPage } from "./routes/LoginPage";
import { hasEditForm, ResourceListPage } from "./routes/ResourceListPage";
const InboxPage = lazy(() => import("./pages/inbox/InboxPage").then((module) => ({ default: module.InboxPage })));
import { FormRouteHost } from "./components/forms/FormRoute";
import type { FormTarget } from "./components/forms/FormRoute";
import { ActivityPage } from "./routes/ActivityPage";
import { GrafanaPage } from "./pages/grafana/GrafanaPage";
import { HealthPage } from "./pages/health/HealthLog";
const SecretFormPage = lazy(() => import("./pages/secrets/SecretFormPage").then((module) => ({ default: module.SecretFormPage })));
const ApprovalsPage = lazy(() => import("./routes/ApprovalsPage").then((module) => ({ default: module.ApprovalsPage })));
const McpElicitationPage = lazy(() => import("./pages/mcp/McpElicitationPage").then((module) => ({ default: module.McpElicitationPage })));
const McpConfirmationPage = lazy(() => import("./pages/mcp/McpElicitationPage").then((module) => ({ default: module.McpConfirmationPage })));
const ApprovalDetailPage = lazy(() => import("./routes/ApprovalDetailPage").then((module) => ({ default: module.ApprovalDetailPage })));
const ModelsPage = lazy(() => import("./pages/models/ModelsPage").then((module) => ({ default: module.ModelsPage })));
const ModelsList = lazy(() => import("./pages/models/ModelsList").then((module) => ({ default: module.ModelsList })));
const ModelPage = lazy(() => import("./pages/models/ModelPage").then((module) => ({ default: module.ModelPage })));
const ModelCanvas = lazy(() => import("./pages/models/ModelCanvas").then((module) => ({ default: module.ModelCanvas })));
const ExplorePage = lazy(() => import("./pages/explore/ExplorePage").then((module) => ({ default: module.ExplorePage })));
const CkanPage = lazy(() => import("./pages/ckan/CkanPage").then((module) => ({ default: module.CkanPage })));
const AccessibilityPage = lazy(() =>
  import("./pages/accessibility/AccessibilityPage").then((module) => ({ default: module.AccessibilityPage })),
);
const GlossaryPage = lazy(() => import("./pages/glossary/GlossaryPage").then((module) => ({ default: module.GlossaryPage })));
const ConnectPage = lazy(() => import("./pages/connect/ConnectPage").then((module) => ({ default: module.ConnectPage })));
import { HomePage } from "./pages/home/HomePage";
const KnowledgePage = lazy(() => import("./pages/knowledge/KnowledgePage").then((module) => ({ default: module.KnowledgePage })));
const SourcePage = lazy(() => import("./pages/knowledge/SourcePage").then((module) => ({ default: module.SourcePage })));
const ConversationsPage = lazy(() =>
  import("./pages/knowledge/ConversationsPage").then((module) => ({ default: module.ConversationsPage })),
);
const AiConnectorsPage = lazy(() => import("./pages/mcp/AiConnectorsPage").then((module) => ({ default: module.AiConnectorsPage })));
const SettingsPage = lazy(() => import("./pages/settings/SettingsPage").then((module) => ({ default: module.SettingsPage })));
const McpServersPage = lazy(() => import("./pages/mcp/McpServersPage").then((module) => ({ default: module.McpServersPage })));
const CataloguePage = lazy(() => import("./pages/catalogue/CataloguePage").then((module) => ({ default: module.CataloguePage })));
const EndpointDatasetPage = lazy(() =>
  import("./pages/catalogue/EndpointDatasetPage").then((module) => ({ default: module.EndpointDatasetPage })),
);
const DatasetPage = lazy(() => import("./pages/catalogue/DatasetPage").then((module) => ({ default: module.DatasetPage })));
import { parseCatalogueSearch } from "./pages/catalogue/search";
const EndpointDocs = lazy(() => import("./pages/endpoints/EndpointDocs").then((module) => ({ default: module.EndpointDocs })));
const PublicViewPage = lazy(() => import("./pages/spaces/PublicView").then((module) => ({ default: module.PublicViewPage })));
const PublicFormPage = lazy(() => import("./pages/spaces/FormView").then((module) => ({ default: module.PublicFormPage })));
import { useAuth } from "./auth/AuthProvider";
const ProjectSettingsPage = lazy(() =>
  import("./pages/projectSettings/ProjectSettingsPage").then((module) => ({ default: module.ProjectSettingsPage })),
);
const ImportPage = lazy(() => import("./pages/import/ImportPage").then((module) => ({ default: module.ImportPage })));
// A page that carries a map, a grid or an editor loads with its route, not with the Portal: the
// entry every visit downloads first was 1.7 MB gzipped while these sat in it (T-3280).
const SpaceInside = lazy(() => import("./pages/spaces/SpaceInside").then((module) => ({ default: module.SpaceInside })));
const UploadCsv = lazy(() => import("./pages/spaces/UploadCsv").then((module) => ({ default: module.UploadCsv })));
const AppPage = lazy(() => import("./pages/apps/AppPage").then((module) => ({ default: module.AppPage })));
const AppOpenPage = lazy(() => import("./pages/apps/AppOpenPage").then((module) => ({ default: module.AppOpenPage })));
const GroupPage = lazy(() => import("./pages/access/GroupPage").then((module) => ({ default: module.GroupPage })));
const AgentPage = lazy(() => import("./pages/organization/Agents").then((module) => ({ default: module.AgentPage })));
const OrganizationPage = lazy(() => import("./pages/organization/OrganizationPage").then((module) => ({ default: module.OrganizationPage })));
const PersonPage = lazy(() => import("./pages/organization/People").then((module) => ({ default: module.PersonPage })));
const EndpointPage = lazy(() => import("./pages/endpoints/EndpointPage").then((module) => ({ default: module.EndpointPage })));
const RegistrationPage = lazy(() => import("./routes/RegistrationPage").then((module) => ({ default: module.RegistrationPage })));
const PipelineOverview = lazy(() =>
  import("./pages/pipelines/PipelineOverview").then((module) => ({ default: module.PipelineOverview })),
);
const MqttBrokerPage = lazy(() =>
  import("./pages/mqttbrokers/MqttBrokerPage").then((module) => ({ default: module.MqttBrokerPage })),
);
const AssistantPage = lazy(() => import("./pages/assistant/AssistantPage").then((module) => ({ default: module.AssistantPage })));
import { HandOff } from "./assistant/HandOff";
import { hasPrefill } from "./assistant/state";
import { DraftElsewhere } from "./assistant/DraftElsewhere";
import { WorkspaceProvider } from "./components/layout/WorkspaceContext";
const WorkspacesPage = lazy(() => import("./routes/WorkspacesPage").then((module) => ({ default: module.WorkspacesPage })));
const ComparePage = lazy(() => import("./pages/workspaces/ComparePage").then((module) => ({ default: module.ComparePage })));
const BringBackPage = lazy(() => import("./pages/workspaces/BringBackPage").then((module) => ({ default: module.BringBackPage })));
const TryItPage = lazy(() => import("./pages/workspaces/TryItPage").then((module) => ({ default: module.TryItPage })));
const Gallery = lazy(() => import("./pages/gallery/Gallery").then((module) => ({ default: module.Gallery })));
import { ORG_NAMESPACE } from "./api/manifest";
import { AdminNav, isOrganizationTab } from "./pages/organization/AdminNav";
import type { OrganizationTab } from "./pages/organization/AdminNav";
import { isProjectSettingsTab } from "./pages/projectSettings/tabs";
import type { ProjectSettingsTab } from "./pages/projectSettings/tabs";
import type { AuthState } from "./auth/AuthProvider";

export interface RouterContext {
  auth: AuthState;
}

/** The page around anything the Portal shows with no project to hang a shell on. */
function Bare({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="flex min-h-screen flex-col bg-bg font-sans text-fg">
      <header className="flex h-14 items-center border-b border-border bg-surface px-4">
        <BrandMark short />
      </header>
      <main id="main" className="flex flex-1 items-center justify-center p-6">{children}</main>
    </div>
  );
}

/**
 * What a route without a `$project` shows while the project list is on its way, when the read
 * fails, and when the repository holds none (PF-05).
 *
 * The three were two: a list that came back 403, or did not come back at all, is not a pending
 * list, so it fell through to "the repository holds no project" — the Portal telling somebody
 * whose token had expired that their organisation's work was gone. A failed read says what the
 * API said and offers the one thing that can help (UI-15).
 */
function NoProject({
  projects,
}: {
  projects: Pick<ReturnType<typeof useProjects>, "isPending" | "isError" | "error" | "errorUpdateCount" | "refetch">;
}): React.JSX.Element {
  const { t } = useTranslation();
  // When the list failed, the project last worked in keeps the shell: the navigation, the
  // person's menu and a way elsewhere stay (T-4165, photo audit T-4163). The shell stays while
  // the list is asked again too: its project switcher asks on mount, and a shell that left for
  // the wait would come back and ask again, for ever.
  const last = projects.errorUpdateCount > 0 ? lastProject() : undefined;
  if (projects.isPending && last) {
    return (
      <Shell project={last}>
        <PageLoading label={t("projects.loading")} />
      </Shell>
    );
  }
  if (projects.isPending) {
    return (
      <Bare>
        <p role="status" className="text-body text-fg-muted">
          {t("projects.loading")}
        </p>
      </Bare>
    );
  }
  if (projects.isError) {
    const failed = (
      <PageFailed
        error={projects.error}
        onRetry={() => {
          void projects.refetch();
        }}
      />
    );
    return last ? (
      <Shell project={last}>{failed}</Shell>
    ) : (
      <Bare>
        <div className="w-full max-w-lg">{failed}</div>
      </Bare>
    );
  }
  return (
    <Bare>
      <EmptyState
        icon="spaces"
        title={t("projects.empty.title")}
        description={t("projects.empty.description")}
      />
    </Bare>
  );
}

/**
 * An address the Portal has no page for (UI-15).
 *
 * Without this the router falls back to its own built-in "Not Found" — two untranslated words
 * on a white page, outside the Portal's chrome, with no way back — which is what a mistyped or
 * an outdated link led to.
 */
function NotFound(): React.JSX.Element {
  // From the router's context, which every page has, rather than `useAuth`, which a page drawn
  // outside the AuthProvider has not.
  const status = (useRouteContext({ strict: false }) as Partial<RouterContext>).auth?.status;
  // A signed-in person keeps the shell of the project last worked in: its navigation, search and
  // menu are the ways on that a bare card left out (T-4165, photo audit T-4157).
  const last = status === "authenticated" ? lastProject() : undefined;
  return last ? (
    <Shell project={last}>
      <NotFoundState />
    </Shell>
  ) : (
    <Bare>
      <NotFoundState />
    </Bare>
  );
}

/** `/` goes to the spaces of the project last worked in, else the first visible one (T-2753). */
function IndexRedirect(): React.JSX.Element {
  const projects = useProjects();
  const first = usePreferredProject();
  // A member (ADR-N-067, T-6411): signed in with no project, so the catalogue is home.
  if (!first && projects.isSuccess) {
    return <Navigate to="/catalogue" />;
  }
  if (!first) {
    return <NoProject projects={projects} />;
  }
  return <Navigate to="/projects/$project/home" params={{ project: first }} />;
}

const rootRoute = createRootRouteWithContext<RouterContext>()({
  // A signed-in page waits here for the shell's chunk (T-5122), drawn as the empty page it becomes.
  component: function Root() {
    return (
      <Suspense fallback={<main aria-busy="true" className="min-h-screen bg-bg" />}>
        <Outlet />
      </Suspense>
    );
  },
});

/**
 * The frame of the public catalogue (EP-81): the Portal's shell for a person who is signed in and
 * works in a project, the brand and a sign-in button for everybody else. The page itself needs
 * no session, so a link to it can be handed to a citizen.
 */
function CatalogueFrame({
  children,
}: {
  /** The page, given the project the signed-in person works in, or `null` for a visitor. */
  children: (project: string | null) => React.ReactNode;
}): React.JSX.Element {
  const { status } = useAuth();
  // Until the session is known the frame is not drawn: a page drawn in one frame and moved into
  // the other remounts, and whatever a person had started typing is gone.
  if (status === "loading") {
    return <main aria-busy="true" className="min-h-screen bg-bg" />;
  }
  if (status === "authenticated") {
    return <SignedInCatalogue>{children}</SignedInCatalogue>;
  }
  return <PublicFrame>{children(null)}</PublicFrame>;
}

function SignedInCatalogue({
  children,
}: {
  children: (project: string | null) => React.ReactNode;
}): React.JSX.Element {
  const projects = useProjects();
  const first = usePreferredProject();
  if (projects.isPending) {
    return <main aria-busy="true" className="min-h-screen bg-bg" />;
  }
  if (first) {
    return <Shell project={first}>{children(first)}</Shell>;
  }
  // A member with no project gets the member shell (ADR-N-067, T-6411); a project list that
  // failed keeps the public frame, and the catalogue still reads.
  return projects.isSuccess ? <MemberFrame>{children(null)}</MemberFrame> : <PublicFrame>{children(null)}</PublicFrame>;
}

/** Every public dataset of the installation, readable without signing in (EP-81). */
const catalogueRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/catalogue",
  validateSearch: parseCatalogueSearch,
  component: function CatalogueRoute() {
    const search = catalogueRoute.useSearch();
    const navigate = catalogueRoute.useNavigate();
    return (
      <CatalogueFrame>
        {(project) => (
          <CataloguePage
            project={project}
            search={search}
            onSearch={(next) => void navigate({ search: next })}
          />
        )}
      </CatalogueFrame>
    );
  },
});

/**
 * One dataset by its Endpoint, read live with the person's own session (ADR-N-067 §2.4,
 * T-6411): the grid shows "sign in" or "no access" where the Endpoint refuses (T-6376).
 */
const catalogueEndpointRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/catalogue/endpoint/$slug",
  component: function CatalogueEndpointRoute() {
    const { slug } = catalogueEndpointRoute.useParams();
    return <CatalogueFrame>{() => <EndpointDatasetPage slug={slug} />}</CatalogueFrame>;
  },
});

/** One public dataset: its resources, its model, a sample and how to use it (EP-82). */
const catalogueDatasetRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/catalogue/$name",
  component: function CatalogueDatasetRoute() {
    const { name } = catalogueDatasetRoute.useParams();
    return (
      <CatalogueFrame>{() => <DatasetPage name={name} />}</CatalogueFrame>
    );
  },
});

/** A project's home: the first-run checklist and what the person's role does next (T-3233, T-3235). */
const homeRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/home",
  // `?welcome=1` is where an invitation's link ends (PF-108): the page greets the newcomer.
  validateSearch: (search: Record<string, unknown>): { welcome?: boolean } =>
    search.welcome === 1 || search.welcome === "1" || search.welcome === true ? { welcome: true } : {},
  component: function HomeRoute() {
    const { project } = homeRoute.useParams();
    const { welcome } = homeRoute.useSearch();
    const navigate = homeRoute.useNavigate();
    return (
      <Shell project={project}>
        <HomePage project={project} welcome={welcome === true} onWelcomeClosed={() => void navigate({ search: {} })} />
      </Shell>
    );
  },
});

/** The platform's words with an example each (T-3236): public, every `Term` links here. */
const glossaryRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/glossary",
  component: function GlossaryRoute() {
    return <CatalogueFrame>{() => <GlossaryPage />}</CatalogueFrame>;
  },
});

/** The accessibility statement (T-5108, Decree 78/2020): public, linked from every frame. */
const accessibilityRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/accessibility",
  component: function AccessibilityRoute() {
    return <CatalogueFrame>{() => <AccessibilityPage />}</CatalogueFrame>;
  },
});

/** A public named MCP server's setup page (T-4169): its tools and a setup per AI client, no sign-in. */
const connectRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/connect/$project/$name",
  component: function ConnectRoute() {
    const { project, name } = connectRoute.useParams();
    // Anonymous for everyone: the page shows what a stranger's AI client reaches.
    return <PublicFrame anonymous>{<ConnectPage project={project} name={name} />}</PublicFrame>;
  },
});

/** A data view published as a public link (T-3108, API/01 §33): read-only, no sign-in. */
const publicViewRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/v/$slug",
  component: function PublicViewRoute() {
    const { slug } = publicViewRoute.useParams();
    return <PublicFrame>{<PublicViewPage slug={slug} />}</PublicFrame>;
  },
});

/** A public Endpoint's API documentation, generated from its OpenAPI (EP-99, T-3265): no sign-in. */
const endpointDocsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/d/$slug",
  component: function EndpointDocsRoute() {
    const { slug } = endpointDocsRoute.useParams();
    return <PublicFrame>{<EndpointDocs slug={slug} open />}</PublicFrame>;
  },
});

/** A form view published as a public form (T-3103, API/01 §33): anyone creates, no sign-in. */
const publicFormRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/f/$slug",
  component: function PublicFormRoute() {
    const { slug } = publicFormRoute.useParams();
    // Anonymous for everyone (EP-102): signing in would change nothing the form does.
    return <PublicFrame anonymous>{<PublicFormPage slug={slug} />}</PublicFrame>;
  },
});

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/login",
  component: LoginPage,
});

/** Everything below this route needs a live session (CC-42, defence in depth). */
const protectedRoute = createRoute({
  getParentRoute: () => rootRoute,
  id: "protected",
  beforeLoad: ({ context, location }) => {
    if (context.auth.status === "anonymous") {
      throw redirect({ to: "/login", search: { redirect_to: location.href } });
    }
  },
  // `?workspace=` on any page reads and writes that copy (UI-61, CC-76): declared here, so every
  // page below carries it in its search type and a link into a copy needs no cast (T-1488).
  // `?org=` is the Organization the switcher shows (PF-124): a name, else nothing.
  validateSearch: (search: Record<string, unknown>): { workspace?: string; org?: string } => ({
    workspace:
      typeof search.workspace === "string" && search.workspace !== "" ? search.workspace : undefined,
    org: typeof search.org === "string" && ORG_NAME.test(search.org) ? search.org : undefined,
  }),
  // And every link below keeps it, until Leave or another project says otherwise (T-4868); the
  // chosen Organization likewise, until the switcher says otherwise.
  search: { middlewares: [keepCopy, keepOrganization] },
  component: function ProtectedRoute() {
    return (
      <WorkspaceProvider>
        <Outlet />
      </WorkspaceProvider>
    );
  },
});

const workspacesRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/workspaces",
  component: function WorkspacesRoute() {
    const { project } = workspacesRoute.useParams();
    return (
      <Shell project={project}>
        <WorkspacesPage project={project} />
      </Shell>
    );
  },
});

/** The "Work on a copy" dialog over the list, so the address a "new" link takes works (T-2749). */
const workspacesNewRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/workspaces/new",
  component: function WorkspacesNewRoute() {
    const { project } = workspacesNewRoute.useParams();
    return (
      <Shell project={project}>
        <WorkspacesPage project={project} creating />
      </Shell>
    );
  },
});

/** The secret form a plan's needsSecret links to (T-5971): a credential is typed here, never in a chat. */
const secretNewRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/secrets/new",
  validateSearch: (search: Record<string, unknown>): { name?: string; key?: string } => ({
    name: typeof search.name === "string" ? search.name : undefined,
    key: typeof search.key === "string" ? search.key : undefined,
  }),
  component: function SecretNewRoute() {
    const { project } = secretNewRoute.useParams();
    const { name, key } = secretNewRoute.useSearch();
    return (
      <Shell project={project}>
        <SecretFormPage project={project} name={name} secretKey={key} />
      </Shell>
    );
  },
});

const workspaceCompareRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/workspaces/$name/compare",
  component: function WorkspaceCompareRoute() {
    const { project, name } = workspaceCompareRoute.useParams();
    return (
      <Shell project={project}>
        <ComparePage project={project} name={name} />
      </Shell>
    );
  },
});

const workspaceTryItRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/workspaces/$name/try-it",
  component: function WorkspaceTryItRoute() {
    const { project, name } = workspaceTryItRoute.useParams();
    return (
      <Shell project={project}>
        <TryItPage project={project} name={name} />
      </Shell>
    );
  },
});

const workspaceBringBackRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/workspaces/$name/bring-back",
  component: function WorkspaceBringBackRoute() {
    const { project, name } = workspaceBringBackRoute.useParams();
    return (
      <Shell project={project}>
        <BringBackPage project={project} name={name} />
      </Shell>
    );
  },
});

const indexRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/",
  component: IndexRedirect,
});

const activityRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/activity",
  validateSearch: (search: Record<string, unknown>): { kind?: string } => ({
    kind: typeof search.kind === "string" && search.kind !== "" ? search.kind : undefined,
  }),
  component: function ActivityRoute() {
    const { project } = activityRoute.useParams();
    const { kind } = activityRoute.useSearch();
    return (
      <Shell project={project}>
        <ActivityPage project={project} kind={kind} />
      </Shell>
    );
  },
});

/** A person's visualizations in the project's Grafana (T-6509). */
const grafanaRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/grafana",
  component: function GrafanaRoute() {
    const { project } = grafanaRoute.useParams();
    return (
      <Shell project={project}>
        <GrafanaPage project={project} />
      </Shell>
    );
  },
});

/** The project's health log: every check of its resources, errors first (T-5067). */
const healthRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/health",
  component: function HealthRoute() {
    const { project } = healthRoute.useParams();
    return (
      <Shell project={project}>
        <HealthPage project={project} />
      </Shell>
    );
  },
});

const approvalsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/approvals",
  component: function ApprovalsRoute() {
    const { project } = approvalsRoute.useParams();
    return (
      <Shell project={project}>
        <ApprovalsPage project={project} />
      </Shell>
    );
  },
});

const approvalDetailRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/approvals/$id",
  component: function ApprovalDetailRoute() {
    const { project, id } = approvalDetailRoute.useParams();
    return (
      <Shell project={project}>
        <ApprovalDetailPage project={project} id={id} />
      </Shell>
    );
  },
});

/** The question an AI client's MCP call asked, decided by the person (AG-63, T-5699). */
const mcpElicitationRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/mcp-elicitations/$id",
  component: function McpElicitationRoute() {
    const { project, id } = mcpElicitationRoute.useParams();
    return (
      <Shell project={project}>
        <McpElicitationPage project={project} id={id} />
      </Shell>
    );
  },
});

/**
 * A context broker's MCP write, decided by the person (T-5829): the broker's question names no
 * project, so the page opens in the project the person works in, or in the public frame.
 */
const mcpConfirmationRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/mcp-confirmations/$id",
  component: function McpConfirmationRoute() {
    const { id } = mcpConfirmationRoute.useParams();
    return <SignedInCatalogue>{() => <McpConfirmationPage id={id} />}</SignedInCatalogue>;
  },
});

/** The federation playground is gone (UI-28): an old link lands on the project home. */
const playgroundRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/playground",
  beforeLoad: () => {
    throw redirect({ to: "/" });
  },
});

/**
 * Every endpoint of every project is the Organization page's Endpoints tab, an administration
 * view (PF-61, T-2877); the old address keeps opening it, and the tab sends anyone else to
 * Settings.
 */
const allEndpointsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/endpoints",
  beforeLoad: () => {
    throw redirect({ to: "/organization/$tab", params: { tab: "endpoints" } });
  },
});

/**
 * Project → Access is folded into Project settings (T-2606, Architecture/09 §14.3): a saved link,
 * a chat message or an older assistant answer lands on Members with its query string, so a
 * `?grant=` hand-off still opens its form.
 */
const accessRedirectRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/access",
  beforeLoad: ({ params, location }) => {
    throw redirect({
      href: `/projects/${encodeURIComponent(params.project)}/settings/members${location.searchStr}`,
      replace: true,
    });
  },
});

/** A bare project address opens the project's home (T-5015); protectedRoute keeps the sign-in check. */
const projectIndexRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project",
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/projects/$project/home", params: { project: params.project }, replace: true });
  },
});

/** Project settings opens on General. */
const projectSettingsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/settings",
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/projects/$project/settings/$tab", params: { project: params.project, tab: "general" } });
  },
});

/** The kind a tab of Project settings creates and edits through its routed form (T-2474). */
const SETTINGS_FORMS: Partial<Record<ProjectSettingsTab, string>> = {
  members: "rolebindings",
  roles: "roles",
  "service-accounts": "serviceaccounts",
};

function ProjectSettingsView({
  project,
  tab,
  form,
  endpoint,
}: {
  project: string;
  tab: string;
  form: FormTarget | null;
  endpoint?: string;
}): React.JSX.Element {
  if (!isProjectSettingsTab(tab)) {
    return <NotFound />;
  }
  return (
    <Shell project={project}>
      <HandOff>
        <FormRouteHost
          project={project}
          plural={SETTINGS_FORMS[tab] ?? tab}
          base={`/projects/${encodeURIComponent(project)}/settings/${tab}`}
          form={form}
        >
          <ProjectSettingsPage project={project} tab={tab} endpoint={endpoint} />
        </FormRouteHost>
      </HandOff>
    </Shell>
  );
}

/**
 * `?endpoint=<name>` opens Your access on that Endpoint's Data access table (T-5593); a name the
 * caller's list does not hold is ignored there, and one that is no resource name is dropped here.
 */
export function settingsSearch(search: Record<string, unknown>): { endpoint?: string } {
  const endpoint = search.endpoint;
  return { endpoint: typeof endpoint === "string" && /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/.test(endpoint) ? endpoint : undefined };
}

/** Project settings, one tab per address (T-2606, UI-76). */
const projectSettingsTabRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/settings/$tab",
  validateSearch: settingsSearch,
  component: function ProjectSettingsTabRoute() {
    const { project, tab } = projectSettingsTabRoute.useParams();
    const { endpoint } = projectSettingsTabRoute.useSearch();
    return <ProjectSettingsView project={project} tab={tab} form={null} endpoint={endpoint} />;
  },
});

/** A tab's form as a page of its own: `…/new`, `…/{name}/edit` (T-2474, UI-27). */
const projectSettingsFormRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/settings/$tab/$",
  validateSearch: settingsSearch,
  component: function ProjectSettingsFormRoute() {
    const { project, tab, _splat: rest = "" } = projectSettingsFormRoute.useParams();
    const { endpoint } = projectSettingsFormRoute.useSearch();
    // The splat matches an empty rest too, and the router may pick it for the tab's own address.
    const form = formOfRest(rest);
    return rest === "" || form !== null ? (
      <ProjectSettingsView project={project} tab={tab} form={form} endpoint={endpoint} />
    ) : (
      <NotFound />
    );
  },
});

/** `/organization` opens on its first tab (Architecture/09 §14.1). */
const organizationRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/organization",
  beforeLoad: () => {
    throw redirect({ to: "/organization/$tab", params: { tab: "settings" } });
  },
});

/** The kind a tab of the Organization page creates and edits through its routed form (T-2474). */
const ORGANIZATION_FORMS: Partial<Record<OrganizationTab, string>> = {
  members: "rolebindings",
  roles: "roles",
  groups: "groups",
  "service-accounts": "serviceaccounts",
};

function OrganizationView({ tab, form }: { tab: string; form: FormTarget | null }): React.JSX.Element {
  const projects = useProjects();
  const first = usePreferredProject();
  if (!isOrganizationTab(tab)) {
    return <NotFound />;
  }
  if (!first) {
    return <NoProject projects={projects} />;
  }
  return (
    <Shell project={first} sidebar={(close) => <AdminNav project={first} active={tab} onNavigate={close} />}>
      <HandOff>
        <FormRouteHost
          project={ORG_NAMESPACE}
          plural={ORGANIZATION_FORMS[tab] ?? tab}
          base={`/organization/${tab}`}
          form={form}
        >
          <OrganizationPage tab={tab} anchor={first} />
        </FormRouteHost>
      </HandOff>
    </Shell>
  );
}

/** The Organization page, one tab per address (T-2605, UI-75): outside any project. */
const organizationTabRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/organization/$tab",
  component: function OrganizationTabRoute() {
    const { tab } = organizationTabRoute.useParams();
    return <OrganizationView tab={tab} form={null} />;
  },
});

/** A tab's form as a page of its own: `…/new`, `…/{name}/edit` (T-2474, UI-27). */
const organizationFormRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/organization/$tab/$",
  component: function OrganizationFormRoute() {
    const { tab, _splat: rest = "" } = organizationFormRoute.useParams();
    const group = tab === "groups" ? groupOfRest(rest) : null;
    if (group !== null) {
      return <OrganizationGroupView name={group} />;
    }
    const person = tab === "people" ? groupOfRest(rest) : null;
    if (person !== null) {
      return <OrganizationPersonView id={person} />;
    }
    if (tab === "agents" && rest !== "") {
      return <OrganizationAgentView path={rest} />;
    }
    const form = formOfRest(rest);
    return rest === "" || form !== null ? <OrganizationView tab={tab} form={form} /> : <NotFound />;
  },
});

/** `groups/{name}`: one group's page (PF-95); `new` stays the new-group form. */
function groupOfRest(rest: string): string | null {
  const [first, second] = rest.split("/");
  return first && first !== "new" && second === undefined ? decodeURIComponent(first) : null;
}

/** A group's page in the Organization's shell (PF-95, T-2685). */
function OrganizationGroupView({ name }: { name: string }): React.JSX.Element {
  const projects = useProjects();
  const first = usePreferredProject();
  if (!first) {
    return <NoProject projects={projects} />;
  }
  return (
    <Shell project={first} sidebar={(close) => <AdminNav project={first} active="groups" onNavigate={close} />}>
      <GroupPage name={name} />
    </Shell>
  );
}

/** An agent's page in the Organization's shell (AG-128, T-4849): `agents/{project}/{kind}/{name}`. */
function OrganizationAgentView({ path }: { path: string }): React.JSX.Element {
  const projects = useProjects();
  const first = usePreferredProject();
  if (!first) {
    return <NoProject projects={projects} />;
  }
  return (
    <Shell project={first} sidebar={(close) => <AdminNav project={first} active="agents" onNavigate={close} />}>
      <AgentPage path={path} />
    </Shell>
  );
}

/** A person's page in the Organization's shell (PF-93, T-2684); `new` stays the new-person form. */
function OrganizationPersonView({ id }: { id: string }): React.JSX.Element {
  const projects = useProjects();
  const first = usePreferredProject();
  if (!first) {
    return <NoProject projects={projects} />;
  }
  return (
    <Shell project={first} sidebar={(close) => <AdminNav project={first} active="people" onNavigate={close} />}>
      <PersonPage id={id} anchor={first} />
    </Shell>
  );
}

/** `new` or `{name}/edit` after a tab's address; anything else names no form. */
function formOfRest(rest: string): FormTarget | null {
  const [first, second, third] = rest.split("/");
  if (first === "new" && second === undefined) return { mode: "new" };
  if (first && second === "edit" && third === undefined) return { mode: "edit", name: decodeURIComponent(first) };
  return null;
}

/**
 * The Data models page (T-2765): the list of the project's models, or the editor when the address
 * asks for one — `?edit=<name>` (AG-77), `?draft=<name>` (AG-61), `?new=blank|file|sdm` with an
 * optional `&space=` — or when a file or the assistant handed the page a draft to open.
 */
const modelsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/models",
  validateSearch: (
    search: Record<string, unknown>,
  ): { edit?: string; draft?: string; new?: "blank" | "file" | "sdm"; space?: string } => ({
    edit: typeof search.edit === "string" && search.edit !== "" ? search.edit : undefined,
    draft: typeof search.draft === "string" && search.draft !== "" ? search.draft : undefined,
    new: search.new === "blank" || search.new === "file" || search.new === "sdm" ? search.new : undefined,
    space: typeof search.space === "string" && search.space !== "" ? search.space : undefined,
  }),
  component: function ModelsRoute() {
    const { project } = modelsRoute.useParams();
    const search = modelsRoute.useSearch();
    const editing =
      search.edit !== undefined ||
      search.draft !== undefined ||
      search.new !== undefined ||
      hasPrefill(`/projects/${project}/models`);
    return (
      <Shell project={project}>
        <HandOff>
          <DraftElsewhere project={project} page="models" />
          {editing ? (
            <ModelsPage key={`${search.edit ?? ""}-${search.new ?? ""}`} project={project} />
          ) : (
            <ModelsList project={project} />
          )}
        </HandOff>
      </Shell>
    );
  },
});

/** One model's own page: its diagram, form, YAML, users, history and Mappings (T-2765). */
const modelRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/models/$name",
  // A link that names a type opens the model on that class (T-2766).
  validateSearch: (search: Record<string, unknown>): { class?: string } => ({
    class: typeof search.class === "string" && search.class !== "" ? search.class : undefined,
  }),
  component: function ModelRoute() {
    const { project, name } = modelRoute.useParams();
    const { class: klass } = modelRoute.useSearch();
    return (
      <Shell project={project}>
        <ModelPage key={`${name}-${klass ?? ""}`} project={project} name={name} initialClass={klass} />
      </Shell>
    );
  },
});

/**
 * One model as a full-screen canvas (T-6094): no Shell, so the diagram takes the whole screen;
 * its own header leads back to the model page.
 */
const modelCanvasRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/models/$name/diagram",
  component: function ModelCanvasRoute() {
    const { project, name } = modelCanvasRoute.useParams();
    return <ModelCanvas key={name} project={project} name={name} />;
  },
});

const exploreRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/explore",
  // The assistant opens the explorer on what it found (UI-46): a space and an endpoint by name,
  // and one entity of them by id (T-1017), so "show me this one" opens the row already selected
  // instead of the list the person then searches by hand. `type` and `q` open the grid already
  // narrowed by the question that was asked (UI-64, UI-67, T-1437); `view=map` opens its map,
  // coloured by the number `colorBy` names, for a request to draw the data (T-6442).
  validateSearch: (
    search: Record<string, unknown>,
  ): {
    space?: string;
    endpoint?: string;
    entityId?: string;
    type?: string;
    q?: string;
    view?: "map";
    colorBy?: string;
  } => ({
    space: typeof search.space === "string" ? search.space : undefined,
    endpoint: typeof search.endpoint === "string" ? search.endpoint : undefined,
    entityId: typeof search.entityId === "string" ? search.entityId : undefined,
    type: typeof search.type === "string" ? search.type : undefined,
    q: typeof search.q === "string" ? search.q : undefined,
    view: search.view === "map" ? "map" : undefined,
    colorBy: typeof search.colorBy === "string" ? search.colorBy : undefined,
  }),
  component: function ExploreRoute() {
    const { project } = exploreRoute.useParams();
    const { space, endpoint, entityId, type, q, view, colorBy } = exploreRoute.useSearch();
    return (
      <Shell project={project}>
        {/*
          The page reads the space and the endpoint only as it mounts, so a second hand-off to
          a card of the same page needs the mount HandOff gives it (T-0793, UI-46).
        */}
        <HandOff>
          <ExplorePage
            project={project}
            initialSpace={space}
            initialEndpoint={endpoint}
            initialEntityId={entityId}
            initialType={type}
            initialQ={q}
            initialView={view}
            colorBy={colorBy}
          />
        </HandOff>
      </Shell>
    );
  },
});

const ckanRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/ckan",
  component: function CkanRoute() {
    const { project } = ckanRoute.useParams();
    return (
      <Shell project={project}>
        <CkanPage project={project} />
      </Shell>
    );
  },
});

/** What the knowledge assistant holds of the project's sources (T-3057, API/01 §34). */
const knowledgeRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/knowledge",
  component: function KnowledgeRoute() {
    const { project } = knowledgeRoute.useParams();
    return (
      <Shell project={project}>
        <KnowledgePage project={project} />
      </Shell>
    );
  },
});

/** What waits for the signed-in person in every project they read (T-3273). */
const inboxRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/inbox",
  component: function InboxRoute() {
    const { project } = inboxRoute.useParams();
    return (
      <Shell project={project}>
        <InboxPage project={project} />
      </Shell>
    );
  },
});

/** The project's named MCP servers over chosen Endpoints (T-3156, ADR-N-043). */
const mcpServersRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/mcp",
  component: function McpServersRoute() {
    const { project } = mcpServersRoute.useParams();
    return (
      <Shell project={project}>
        <McpServersPage project={project} />
      </Shell>
    );
  },
});

/** The old address of a person's connectors (UI-98): they are connector records since T-5970,
 * managed on My AI connectors (T-5951). */
const connectorsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/connectors",
  component: function ConnectorsRoute() {
    return <Navigate to="/ai-connectors" replace />;
  },
});

/** The person's AI connectors at `/c/{id}` (AG-142, T-5951): outside any project, like `/connectors`. */
const aiConnectorsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/ai-connectors",
  component: function AiConnectorsRoute() {
    const projects = useProjects();
    const first = usePreferredProject();
    if (!first && projects.isSuccess) {
      return <MemberFrame><AiConnectorsPage /></MemberFrame>;
    }
    if (!first) {
      return <NoProject projects={projects} />;
    }
    return (
      <Shell project={first}>
        <AiConnectorsPage />
      </Shell>
    );
  },
});

/** Every setting a person reaches, personal, project and platform (owner 2026-11-01). */
const settingsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/settings",
  component: function SettingsRoute() {
    const projects = useProjects();
    const first = usePreferredProject();
    if (!first && projects.isSuccess) {
      return <MemberFrame><SettingsPage /></MemberFrame>;
    }
    if (!first) {
      return <NoProject projects={projects} />;
    }
    return (
      <Shell project={first}>
        <SettingsPage project={first} />
      </Shell>
    );
  },
});

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The assistant's stored conversations, for its administrator (API/01 §34, T-4670). */
const knowledgeConversationsRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/knowledge/conversations",
  // `?open=<id>&compare=<id>`: an eval run's link opens its conversation against the previous run's (T-4681).
  validateSearch: (search: Record<string, unknown>): { open?: string; compare?: string } => ({
    open: typeof search.open === "string" && UUID.test(search.open) ? search.open : undefined,
    compare: typeof search.compare === "string" && UUID.test(search.compare) ? search.compare : undefined,
  }),
  component: function KnowledgeConversationsRoute() {
    const { project } = knowledgeConversationsRoute.useParams();
    const { open, compare } = knowledgeConversationsRoute.useSearch();
    return (
      <Shell project={project}>
        <ConversationsPage project={project} open={open} against={compare} />
      </Shell>
    );
  },
});

/** One source's pages, documents and passages, included and excluded (T-3057). */
const knowledgeSourceRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/knowledge/$source",
  component: function KnowledgeSourceRoute() {
    const { project, source } = knowledgeSourceRoute.useParams();
    return (
      <Shell project={project}>
        <SourcePage project={project} source={source} />
      </Shell>
    );
  },
});

/** Reading a bundle another instance exported into this project (MF-20…MF-24, T-0217). */
const importRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/import",
  component: function ImportRoute() {
    const { project } = importRoute.useParams();
    return (
      <Shell project={project}>
        <ImportPage project={project} />
      </Shell>
    );
  },
});

/** The Federation page is gone (UI-28): an old link lands on the project's context spaces. */
const federationRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/federation",
  beforeLoad: ({ params }) => {
    throw redirect({
      to: "/projects/$project/$plural",
      params: { project: params.project, plural: "spaces" },
    });
  },
});

import { SpaceComplete } from "./pages/spaces/SpaceComplete";

/** Space Complete route (static, so `/spaces/complete` is never read as a space called "complete"). */
const spaceCompleteRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/spaces/complete",
  validateSearch: (search: Record<string, unknown>): { space?: string } =>
    typeof search.space === "string" ? { space: search.space } : {},
  component: function SpaceCompleteRoute() {
    const { project } = spaceCompleteRoute.useParams();
    return (
      <Shell project={project}>
        <HandOff>
          <SpaceComplete project={project} />
        </HandOff>
      </Shell>
    );
  },
});

/** Upload a CSV into a space (DM-81, T-3840); it loads with its page, the entry budget is
 * every visit's (T-3280). */
const spaceUploadRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/spaces/upload",
  validateSearch: (search: Record<string, unknown>): { space?: string } =>
    typeof search.space === "string" ? { space: search.space } : {},
  component: function SpaceUploadRoute() {
    const { project } = spaceUploadRoute.useParams();
    const { space } = spaceUploadRoute.useSearch();
    return (
      <Shell project={project}>
        <UploadCsv project={project} space={space} />
      </Shell>
    );
  },
});

/** The assistant workbench (UI-54). */
const assistantRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/assistant",
  component: function AssistantRoute() {
    const { project } = assistantRoute.useParams();
    return (
      <Shell project={project}>
        <AssistantPage project={project} />
      </Shell>
    );
  },
});

// The shared references had a page of their own; they are rows of the Endpoints grid now
// (T-6169). The old URL still lands there, so a bookmark and the
// assistant's `navigate` are not broken (T-0706, EP-15).
const sharedRedirectRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/shared",
  component: function SharedRedirectRoute() {
    const { project } = sharedRedirectRoute.useParams();
    return (
      <Navigate
        replace
        to="/projects/$project/$plural"
        params={{ project, plural: "endpoints" }}
      />
    );
  },
});

// The Flows page is gone (T-6096): its set-ups start on the Data sources page and "New app".
// A bookmark of the old address lands on the project's home instead of an unknown section.
const flowsRedirectRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/flows",
  component: function FlowsRedirectRoute() {
    const { project } = flowsRedirectRoute.useParams();
    return <Navigate replace to="/projects/$project/home" params={{ project }} />;
  },
});

/** A resource's own page, for the kinds that have one (UI-01, T-2281, AP-68). */
const DETAIL_PAGES = new Set(["spaces", "endpoints", "apps", "csrs", "pipelines", "mqttbrokers"]);

/**
 * One section of a project: its list, one of its forms as a page of its own (T-2474, UI-27), or
 * one resource's page. The list and its forms are one mounted page whichever the address names,
 * so leaving a form returns to the list as it was left, with the Change a proposal opened.
 */
const sectionRoute = createRoute({
  getParentRoute: () => protectedRoute,
  path: "/projects/$project/$plural",
  // `?edit=<name>` opens the kind's own editor on that resource (T-2281): the assistant's hand-off
  // and older links; the endpoint's settings page links the edit address itself.
  // `?draft=<name>` opens the kind's form on that draft (AG-61, AG-73).
  // `?dataType=`, `?dataQ=` and `?dataSort=` hold what an endpoint's Data section shows (T-5082).
  validateSearch: (
    search: Record<string, unknown>,
  ): { edit?: string; draft?: string; dataType?: string; dataQ?: string; dataSort?: string } => {
    const text = (value: unknown) => (typeof value === "string" && value !== "" ? value : undefined);
    return {
      edit: text(search.edit),
      draft: text(search.draft),
      dataType: text(search.dataType),
      dataQ: text(search.dataQ),
      dataSort: text(search.dataSort),
    };
  },
  // One list, one editor (T-4829): the generic DataModel list and its form opened version,
  // source and lifecycle only. Every old address lands on the model page that edits it.
  beforeLoad: ({ params, location, search }) => {
    if (params.plural !== "datamodels") return;
    const [, name, edit] = location.pathname.split(`/projects/${params.project}/datamodels`)[1]?.split("/") ?? [];
    const model = name ? decodeURIComponent(name) : search.edit;
    if (model === "new") {
      throw redirect({ to: "/projects/$project/models", params: { project: params.project }, search: { new: "blank" }, replace: true });
    }
    if (model && (edit === "edit" || search.edit !== undefined)) {
      throw redirect({ to: "/projects/$project/models", params: { project: params.project }, search: { edit: model }, replace: true });
    }
    if (model) {
      throw redirect({ to: "/projects/$project/models/$name", params: { project: params.project, name: model }, replace: true });
    }
    throw redirect({
      to: "/projects/$project/models",
      params: { project: params.project },
      search: search.draft ? { draft: search.draft } : {},
      replace: true,
    });
  },
  component: function SectionRoute() {
    const { project, plural } = sectionRoute.useParams();
    const { edit } = sectionRoute.useSearch();
    const hiddenSections = useHiddenSections();
    const navigate = useNavigate();
    const child = useChildMatches({
      select: (matches) => {
        const first = matches[0];
        return first
          ? { routeId: first.routeId as string, name: (first.params as { name?: string }).name }
          : null;
      },
    });
    // A section this installation hides is no page at any of its addresses, a bookmark
    // included: it goes to the project's spaces, and waits for the branding before drawing
    // anything that might be taken away (T-2874). One navigation per decision, not one per
    // render: `<Navigate>` fires again on every render of the route it leaves.
    const hidden = isHiddenSection(plural, hiddenSections);
    const leave = hidden && hiddenSections !== undefined;
    useEffect(() => {
      if (leave) {
        void navigate({ to: "/projects/$project/$plural", params: { project, plural: "spaces" }, replace: true });
      }
    }, [leave, navigate, project]);
    if (hidden) {
      return null;
    }
    if (child?.routeId === sectionDetailRoute.id && child.name !== undefined) {
      return <DetailPage project={project} plural={plural} name={child.name} />;
    }
    if (child?.routeId === sectionOpenRoute.id && child.name !== undefined) {
      return (
        <Shell project={project} fill>
          <AppOpenPage project={project} name={child.name} />
        </Shell>
      );
    }
    const form: FormTarget | null =
      child?.routeId === sectionNewRoute.id
        ? { mode: "new" }
        : child?.routeId === sectionEditRoute.id && child.name !== undefined
          ? { mode: "edit", name: child.name }
          : null;
    return (
      <Shell project={project}>
        <HandOff>
          <DraftElsewhere project={project} page={plural} />
          <FormRouteHost project={project} plural={plural} form={form}>
            <ResourceListPage
              project={project}
              plural={plural}
              edit={form?.mode === "edit" ? form.name : edit}
            />
          </FormRouteHost>
        </HandOff>
      </Shell>
    );
  },
});

function DetailPage({
  project,
  plural,
  name,
}: {
  project: string;
  plural: string;
  name: string;
}): React.JSX.Element {
  return (
    <Shell project={project}>
      {plural === "spaces" ? (
        <SpaceInside project={project} name={name} />
      ) : plural === "endpoints" ? (
        <EndpointPage project={project} name={name} />
      ) : plural === "csrs" ? (
        <RegistrationPage project={project} name={name} />
      ) : plural === "pipelines" ? (
        <PipelineOverview project={project} name={name} />
      ) : plural === "mqttbrokers" ? (
        <MqttBrokerPage project={project} name={name} />
      ) : (
        <AppPage project={project} name={name} />
      )}
    </Shell>
  );
}

/** Rendered by the section: the children only name which of its views the address asks for. */
function Nothing(): null {
  return null;
}

const sectionIndexRoute = createRoute({
  getParentRoute: () => sectionRoute,
  path: "/",
  component: Nothing,
});

/** A kind's create form, at `/projects/{project}/{plural}/new` (T-2474). */
const sectionNewRoute = createRoute({
  getParentRoute: () => sectionRoute,
  path: "new",
  component: Nothing,
});

/** One resource's edit form, at `/projects/{project}/{plural}/{name}/edit` (T-2474). */
const sectionEditRoute = createRoute({
  getParentRoute: () => sectionRoute,
  path: "$name/edit",
  component: Nothing,
});

/**
 * What one Context Space holds, one endpoint's settings, an application and its runs; `new` is
 * the create form above, so a resource called `new` has only its edit address.
 */
const sectionDetailRoute = createRoute({
  getParentRoute: () => sectionRoute,
  path: "$name",
  beforeLoad: ({ params }) => {
    // Every item has this address (T-3241): a kind without a page of its own opens on its
    // form, so a link sent as `/projects/{project}/{plural}/{name}` always lands. What is no
    // kind gets the Portal's own "no such page", not the section's page around it.
    if (!DETAIL_PAGES.has(params.plural)) {
      if (hasEditForm(params.plural)) {
        throw redirect({ to: "/projects/$project/$plural/$name/edit", params, replace: true });
      }
      throw notFound({ routeId: rootRoute.id });
    }
  },
  component: Nothing,
});

/** A published App inside the Portal, at `/projects/{project}/apps/{name}/open` (AP-122). */
const sectionOpenRoute = createRoute({
  getParentRoute: () => sectionRoute,
  path: "$name/open",
  beforeLoad: ({ params }) => {
    if (params.plural !== "apps") {
      throw notFound({ routeId: rootRoute.id });
    }
  },
  component: Nothing,
});

/**
 * The component gallery (T-1729), development only: no session, no project, no API. The route is
 * built only when `import.meta.env.DEV`, which a production build replaces with `false` — the
 * branch goes, the import with it, and `gallery_axe.test.tsx` holds that nothing else imports
 * the module.
 */
const devRoutes = import.meta.env.DEV
  ? [
      createRoute({
        getParentRoute: () => rootRoute,
        path: "/__gallery",
        component: Gallery,
      }),
    ]
  : [];

export const routeTree = rootRoute.addChildren([
  loginRoute,
  catalogueRoute,
  catalogueEndpointRoute,
  catalogueDatasetRoute,
  publicViewRoute,
  publicFormRoute,
  endpointDocsRoute,
  glossaryRoute,
  accessibilityRoute,
  connectRoute,
  ...devRoutes,
  protectedRoute.addChildren([
    indexRoute,
    homeRoute,
    activityRoute,
    grafanaRoute,
    healthRoute,
    approvalsRoute,
    approvalDetailRoute,
    mcpElicitationRoute,
    mcpConfirmationRoute,
    playgroundRoute,
    allEndpointsRoute,
    organizationRoute,
    organizationTabRoute,
    organizationFormRoute,
    accessRedirectRoute,
    projectIndexRoute,
    projectSettingsRoute,
    projectSettingsTabRoute,
    projectSettingsFormRoute,
    modelsRoute,
    modelRoute,
    modelCanvasRoute,
    exploreRoute,
    ckanRoute,
    knowledgeRoute,
    inboxRoute,
    knowledgeConversationsRoute,
    knowledgeSourceRoute,
    mcpServersRoute,
    connectorsRoute,
    aiConnectorsRoute,
    settingsRoute,
    importRoute,
    federationRoute,
    spaceCompleteRoute,
    spaceUploadRoute,
    assistantRoute,
    sharedRedirectRoute,
    flowsRedirectRoute,
    workspacesRoute,
    workspacesNewRoute,
    secretNewRoute,
    workspaceTryItRoute,
    workspaceCompareRoute,
    workspaceBringBackRoute,
    sectionRoute.addChildren([
      sectionIndexRoute,
      sectionNewRoute,
      sectionEditRoute,
      sectionOpenRoute,
      sectionDetailRoute,
    ]),
  ]),
]);

export function createPortalRouter() {
  return createRouter({
    // Written out: the code graph reads no reference in a shorthand property (cg dead-code ratchet).
    routeTree: routeTree,
    context: { auth: undefined as unknown as AuthState },
    defaultPreload: false,
    // Back and forward land where the page was left, not at its top (UI-89, T-3239).
    scrollRestoration: true,
    defaultNotFoundComponent: NotFound,
    // An address no route matches whole is the root's not-found page (T-4817). The default
    // ("fuzzy") hands it to the deepest partial match, and the section route draws its children
    // itself, so `/pipelines/{name}/runs` rendered the `$name` page of another kind.
    notFoundMode: "root",
    // What throws before a page is drawn — a loader, a `beforeLoad` — cannot be caught inside
    // the shell, because there is no shell yet. It gets the same words as the boundary at the
    // root rather than the router's own "Something went wrong!" (T-2426, UI-44).
    defaultErrorComponent: function RouteFailed({ error }: { error: unknown }) {
      return isChunkLoadError(error) ? <ChunkFailed /> : <ErrorPage reference={errorReference()} />;
    },
  });
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof createPortalRouter>;
  }
}
