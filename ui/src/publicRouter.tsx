import { Suspense, lazy, useEffect } from "react";
import { Outlet, createRootRouteWithContext, createRoute, createRouter } from "@tanstack/react-router";
import type { RouterHistory } from "@tanstack/react-router";
import type { QueryClient } from "@tanstack/react-query";
import { PublicFrame } from "./components/layout/PublicFrame";
import { ChunkFailed, ErrorPage, errorReference, isChunkLoadError } from "./components/ErrorBoundary";
import { parseCatalogueSearch } from "./pages/catalogue/search";
import { catalogueQuery, datasetQuery } from "./pages/catalogue/queries";
import { NotFoundState } from "./components/NotFoundState";

/**
 * The pages anybody may open, routed without the signed-in Portal (T-5122): a visitor's first
 * load holds these routes and their frame, not the hundred routes, the shell and every page's
 * words a person who works in a project needs. main.tsx boots this router for a path
 * {@link isPublicPath} accepts; a signed-in person gets the full router there (PublicApp.tsx).
 */
const PUBLIC_PATH = /^\/(catalogue(\/[^/]+)?|glossary|accessibility|v\/[^/]+|d\/[^/]+|f\/[^/]+|connect\/[^/]+\/[^/]+)\/?$/;

export function isPublicPath(path: string): boolean {
  return PUBLIC_PATH.test(path);
}

const CataloguePage = lazy(() => import("./pages/catalogue/CataloguePage").then((module) => ({ default: module.CataloguePage })));
const DatasetPage = lazy(() => import("./pages/catalogue/DatasetPage").then((module) => ({ default: module.DatasetPage })));
const GlossaryPage = lazy(() => import("./pages/glossary/GlossaryPage").then((module) => ({ default: module.GlossaryPage })));
const AccessibilityPage = lazy(() =>
  import("./pages/accessibility/AccessibilityPage").then((module) => ({ default: module.AccessibilityPage })),
);
const ConnectPage = lazy(() => import("./pages/connect/ConnectPage").then((module) => ({ default: module.ConnectPage })));
const PublicViewPage = lazy(() => import("./pages/spaces/PublicView").then((module) => ({ default: module.PublicViewPage })));
const EndpointDocs = lazy(() => import("./pages/endpoints/EndpointDocs").then((module) => ({ default: module.EndpointDocs })));
const PublicFormPage = lazy(() => import("./pages/spaces/FormView").then((module) => ({ default: module.PublicFormPage })));

/**
 * A link out of the public pages (sign-in, a project, the home page): the full Portal draws it, so
 * the browser loads the address again and main.tsx boots the full router for it.
 */
function FullPortal(): React.JSX.Element {
  // A public-looking address no public route draws (`/catalogue/a/b/`) is not found; reloading it
  // would boot this router again, for ever.
  const elsewhere = !isPublicPath(window.location.pathname);
  useEffect(() => {
    if (elsewhere) window.location.reload();
  }, [elsewhere]);
  return elsewhere ? (
    <main aria-busy="true" className="min-h-screen bg-bg" />
  ) : (
    <PublicFrame>
      <NotFoundState />
    </PublicFrame>
  );
}

const rootRoute = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  component: function PublicRoot() {
    return (
      <Suspense fallback={<main aria-busy="true" className="min-h-screen bg-bg" />}>
        <Outlet />
      </Suspense>
    );
  },
});

const catalogueRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/catalogue",
  validateSearch: parseCatalogueSearch,
  // Started, not awaited: the page's chunk loads beside the read instead of after it (T-5165).
  loaderDeps: ({ search }) => search,
  loader: ({ context, deps }) => {
    void context.queryClient.prefetchQuery(catalogueQuery(deps));
  },
  component: function PublicCatalogueRoute() {
    const search = catalogueRoute.useSearch();
    const navigate = catalogueRoute.useNavigate();
    return (
      <PublicFrame>
        <CataloguePage project={null} search={search} onSearch={(next) => void navigate({ search: next })} />
      </PublicFrame>
    );
  },
});

const catalogueDatasetRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/catalogue/$name",
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(datasetQuery(params.name));
  },
  component: function PublicDatasetRoute() {
    const { name } = catalogueDatasetRoute.useParams();
    return (
      <PublicFrame>
        <DatasetPage name={name} />
      </PublicFrame>
    );
  },
});

const glossaryRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/glossary",
  component: () => (
    <PublicFrame>
      <GlossaryPage />
    </PublicFrame>
  ),
});

const accessibilityRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/accessibility",
  component: () => (
    <PublicFrame>
      <AccessibilityPage />
    </PublicFrame>
  ),
});

const connectRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/connect/$project/$name",
  component: function PublicConnectRoute() {
    const { project, name } = connectRoute.useParams();
    return (
      <PublicFrame anonymous>
        <ConnectPage project={project} name={name} />
      </PublicFrame>
    );
  },
});

const publicViewRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/v/$slug",
  component: function PublicViewRoute() {
    const { slug } = publicViewRoute.useParams();
    return (
      <PublicFrame>
        <PublicViewPage slug={slug} />
      </PublicFrame>
    );
  },
});

const endpointDocsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/d/$slug",
  component: function PublicEndpointDocsRoute() {
    const { slug } = endpointDocsRoute.useParams();
    return (
      <PublicFrame>
        <EndpointDocs slug={slug} open />
      </PublicFrame>
    );
  },
});

const publicFormRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/f/$slug",
  component: function PublicFormRoute() {
    const { slug } = publicFormRoute.useParams();
    return (
      <PublicFrame anonymous>
        <PublicFormPage slug={slug} />
      </PublicFrame>
    );
  },
});

/** `history` defaults to the browser's; a test passes a memory one. */
export function createPublicRouter(queryClient: QueryClient, history?: RouterHistory) {
  return createRouter({
    context: { queryClient },
    history,
    routeTree: rootRoute.addChildren([
      catalogueRoute,
      catalogueDatasetRoute,
      glossaryRoute,
      accessibilityRoute,
      connectRoute,
      publicViewRoute,
      endpointDocsRoute,
      publicFormRoute,
    ]),
    defaultPreload: false,
    scrollRestoration: true,
    defaultNotFoundComponent: FullPortal,
    notFoundMode: "root",
    defaultErrorComponent: function PublicRouteFailed({ error }: { error: unknown }) {
      return isChunkLoadError(error) ? <ChunkFailed /> : <ErrorPage reference={errorReference()} />;
    },
  });
}
