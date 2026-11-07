import { lazy, Suspense, useCallback, useId, useMemo, useState } from "react";
import { PermissionGuard } from "../components/ui/PermissionGuard";
import type { JSX } from "react";
import { keepPreviousData, useQueries, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { Feature, FeatureCollection } from "geojson";
import { api, ApiError, queryKeys, unwrap, whilePending } from "../api/client";
import { getDraft } from "../api/drafts";
import { asManifests, localized } from "../api/manifest";
import type { Change, Manifest } from "../api/manifest";
import { takeEditRequest } from "../assistant/state";
import { ChangeNotice } from "../components/ChangeNotice";
import { ResourceRowActions } from "../components/ResourceRowActions";
import { rendersWithDeckGl } from "../components/dashboards/rendering";
import { RAMP } from "../components/dashboards/mapColours";
import type { Bbox, MapLayer } from "../components/dashboards/MapLibreView";
import type { DenseLayer } from "../components/dashboards/DeckGlOverlay";
import { GridWidget } from "../components/dashboards/GridWidget";
import { TypeLink } from "../pages/models/ModelLinks";
import { TemporalChart } from "../components/dashboards/TemporalChart";
import { TypeChart } from "../components/dashboards/TypeChart";
import {
  DashboardEditor,
  dashboardFromManifest,
  LayerEditor,
  layerFromManifest,
} from "../pages/dashboards/editors";
import type { DashboardForm, LayerForm } from "../pages/dashboards/editors";
import {
  Alert,
  Badge,
  Button,
  Card,
  Checkbox,
  EmptyState,
  Icon,
  PageHeader,
  Select,
  Skeleton,
  Tabs,
  tabPanelProps,
} from "../components/ui";

// MapLibre is about half of the bundle; nobody downloads it before opening a dashboard.
const MapLibreView = lazy(() =>
  import("../components/dashboards/MapLibreView").then((module) => ({
    default: module.MapLibreView,
  })),
);

// deck.gl is the other half again, and only a dense dashboard ever needs it.
const DeckGlOverlay = lazy(() =>
  import("../components/dashboards/DeckGlOverlay").then((module) => ({
    default: module.DeckGlOverlay,
  })),
);

/** The styles a Layer manifest may name; `icon` has no renderer yet and draws as a circle. */
const STYLES = ["circle", "line", "fill", "hexagon", "heatmap"] as const;

type LayerStyle = (typeof STYLES)[number];

/** One resolved layer before its features are known, which is what decides the renderer. */
type PlannedLayer = Omit<DenseLayer, "features">;

interface LayerSpec {
  sourceEndpointRef?: string;
  entityType?: string;
  style?: LayerStyle | "icon";
  visible?: boolean;
  filter?: { q?: string; scopeQ?: string; geoQ?: string };
  colorBy?: { property: string; domain?: [number, number]; palette?: string };
  sizeBy?: { property: string; range?: [number, number] };
  popupProperties?: string[];
}

/** A widget of an analytics page (UI-18), as jc-core's `Widget` writes it. */
interface WidgetSpec {
  widgetType: string;
  endpointRef?: string;
  entityId?: string;
  property?: string;
  /**
   * The entity type a `grid` shows (T-1440), or a `bar-chart` or `histogram` counts (UI-93); the
   * grid's own configuration; the filter a chart over a type reads with.
   */
  entityType?: string;
  grid?: Record<string, unknown>;
  q?: string;
}

interface DashboardSpec {
  title?: Record<string, string>;
  visibility?: "private" | "project" | "organization" | "public";
  pages?: { title?: string; layout?: string; layers?: string[]; widgets?: WidgetSpec[] }[];
}

/** What a layer's read asks for: NGSI-LD's own GeoJSON format (ETSI GS CIM 009 §4.5.17, API/02 §3). */
export const GEOJSON_ACCEPT = "application/geo+json";

/**
 * The Endpoint's NGSI-LD API, asked for GeoJSON, is where a layer's features come from (UI-22,
 * API/02 §3; an Endpoint publishes no GeoJSON file since T-4600). The spatial constraint is
 * the manifest's own `geoQ` when it declares one, else the viewport once the reader moved the
 * map, as NGSI-LD's `georel`, `geometry`, `coordinates`.
 */
export function geojsonUrl(slug: string, spec: LayerSpec, bbox?: Bbox): string {
  const params = new URLSearchParams();
  params.set("options", "keyValues");
  // ponytail: one page at the broker's ceiling of 1000 entities; page with `offset` when a
  // layer draws more than that.
  params.set("limit", "1000");
  if (spec.entityType) {
    params.set("type", spec.entityType);
  }
  for (const key of ["q", "scopeQ"] as const) {
    const value = spec.filter?.[key];
    if (value) {
      params.set(key, value);
    }
  }
  if (spec.filter?.geoQ) {
    // `georel` may itself carry a `;` (`near;maxDistance==2000`), so the split is on the
    // three parameter names, not on the separator.
    const parts = spec.filter.geoQ.split(/(?:^|;)(?=(?:georel|geometry|coordinates)=)/);
    for (const part of parts) {
      const at = part.indexOf("=");
      if (at > 0) {
        params.set(part.slice(0, at).trim(), part.slice(at + 1));
      }
    }
  } else if (bbox) {
    const [w, s, e, n] = bbox;
    params.set("georel", "within");
    params.set("geometry", "Polygon");
    params.set("coordinates", JSON.stringify([[[w, s], [e, s], [e, n], [w, n], [w, s]]]));
  }
  return `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities?${params.toString()}`;
}

/**
 * Every layer's features, fetched once (UI-21, UI-22).
 *
 * The count is only knowable from the data, and deck.gl needs the data anyway, so the page
 * fetches what MapLibre would otherwise have fetched for itself and hands it on. A body that
 * is not a FeatureCollection yields `null`: the layer then keeps its URL and MapLibre fetches
 * it the old way, which is the right answer for an Endpoint that answered with a problem
 * document.
 */
async function fetchFeatures(url: string): Promise<Feature[] | null> {
  const response = await fetch(url, { credentials: "same-origin", headers: { Accept: GEOJSON_ACCEPT } });
  if (!response.ok) {
    throw new ApiError(response.status, response.statusText);
  }
  const body = (await response.json()) as FeatureCollection | null;
  return Array.isArray(body?.features) ? body.features : null;
}

/** A dashboard is three lists deep: the dashboard, its layers, and the endpoints they read. */
function useResourceList(project: string, plural: string) {
  return useQuery({
    queryKey: queryKeys.list(project, plural),
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
  });
}

/** The map's frame while its features or its bundle are on the way. */
function MapPlaceholder({ label }: { label: string }): JSX.Element {
  return (
    // The map's height is a fraction of the window, and the spacing scale has no viewport step.
    <div role="status" aria-label={label} className="relative h-[60vh] min-h-80 w-full">
      <Skeleton className="absolute inset-0 rounded-none" />
      <span className="sr-only">{label}</span>
    </div>
  );
}

export function DashboardsPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [selected, setSelected] = useState<string | null>(null);
  const selectId = useId();
  const pagesId = useId();
  // The viewport after the reader moved the map, and the layers switched off in the legend.
  const [bbox, setBbox] = useState<Bbox | undefined>(undefined);
  const [hidden, setHidden] = useState<string[] | null>(null);
  /** The name the open editor's resource carries in the repository, `undefined` for a new one. */
  const [openedAs, setOpenedAs] = useState<string | undefined>(undefined);
  /** Which of the dashboard's pages is open; a dashboard of one page never shows the tabs. */
  const [pageIndex, setPageIndex] = useState(0);
  const [change, setChange] = useState<Change | null>(null);
  const [editingDashboard, setEditingDashboard] = useState<DashboardForm | null>(null);
  const [editingLayer, setEditingLayer] = useState<LayerForm | null>(null);
  const [isNew, setIsNew] = useState(false);
  // The assistant may have sent the person here with a change to one dashboard or layer (AG-77).
  const [request, setRequest] = useState(() => takeEditRequest());
  // Or with a new layer or dashboard it drafted (`?draft=`, AG-73): the address names the draft
  // and not its kind, so the page asks for a Layer first and then for a Dashboard of that name.
  // An `?edit=` hand-off names its draft too, and is the request above already.
  const [handed, setHanded] = useState(() =>
    request !== null || typeof window === "undefined"
      ? undefined
      : (new URLSearchParams(window.location.search).get("draft") || undefined),
  );
  const handedDraft = useQuery({
    queryKey: ["drafts", project, "handed", handed],
    enabled: handed !== undefined,
    queryFn: async () =>
      handed === undefined
        ? null
        : ((await getDraft(project, "Layer", handed)) ?? (await getDraft(project, "Dashboard", handed))),
  });
  if (handed !== undefined && handedDraft.data?.manifest) {
    setHanded(undefined);
    setRequest({ name: handed, manifest: handedDraft.data.manifest as Record<string, unknown> });
  }

  const dashboards = useResourceList(project, "dashboards");
  const layers = useResourceList(project, "layers");
  const endpoints = useResourceList(project, "endpoints");
  const spaces = useResourceList(project, "spaces");
  const models = useResourceList(project, "datamodels");
  const onMoveEnd = useCallback((next: Bbox) => setBbox(next), []);

  const all = useMemo(
    () => asManifests(dashboards.data?.items ?? []).filter((item) => item.kind === "Dashboard"),
    [dashboards.data],
  );

  const dashboard = useMemo(
    () => all.find((item) => item.metadata.name === selected) ?? all[0],
    [all, selected],
  );

  const layerManifests = useMemo(
    () => asManifests(layers.data?.items ?? []).filter((item) => item.kind === "Layer"),
    [layers.data],
  );
  const endpointManifests = useMemo(() => asManifests(endpoints.data?.items ?? []), [endpoints.data]);

  // The lists say whether the resource exists: a dashboard the assistant drafted opens as new.
  const listed = dashboards.data && layers.data
    ? [...all, ...layerManifests]
    : undefined;
  const requested =
    request && listed
      ? ((request.manifest as Manifest | null) ?? listed.find((item) => item.metadata.name === request.name))
      : undefined;
  if (requested && listed) {
    setRequest(null);
    setIsNew(!listed.some((item) => item.kind === requested.kind && item.metadata.name === requested.metadata.name));
    setOpenedAs(requested.metadata.name);
    if (requested.kind === "Layer") {
      setEditingLayer(layerFromManifest(requested));
    } else {
      setSelected(requested.metadata.name);
      setEditingDashboard(dashboardFromManifest(requested));
    }
  }
  // The layers a drafted dashboard draws that are drafts themselves, proposed with it (AG-77).
  const draftedNames = useMemo(() => {
    const known = new Set(layerManifests.map((item) => item.metadata.name));
    const drawn = (editingDashboard?.pages ?? []).flatMap((page) => page.layers ?? []);
    return [...new Set(drawn)].filter((name) => !known.has(name));
  }, [editingDashboard, layerManifests]);
  const drafts = useQueries({
    queries: draftedNames.map((name) => ({
      queryKey: ["drafts", project, "Layer", name],
      queryFn: () => getDraft(project, "Layer", name),
    })),
  });
  const draftedLayers = drafts.flatMap((query) => (query.data ? [query.data.manifest as unknown as Manifest] : []));
  const spaceManifests = useMemo(() => asManifests(spaces.data?.items ?? []), [spaces.data]);
  const modelManifests = useMemo(() => asManifests(models.data?.items ?? []), [models.data]);

  const { mapLayers, blocked, legend, widgets } = useMemo(() => {
    const spec = (dashboard?.spec ?? {}) as DashboardSpec;
    const pages = spec.pages ?? [];
    // A dashboard with a second page used to end at the first one (T-0792): the reader
    // chooses the page, and everything below is that page's.
    const page = pages[Math.min(pageIndex, Math.max(pages.length - 1, 0))];
    const layerByName = new Map(layerManifests.map((item) => [item.metadata.name, item] as const));
    const endpointByName = new Map(endpointManifests.map((item) => [item.metadata.name, item] as const));

    const resolved: PlannedLayer[] = [];
    const refused: string[] = [];
    const listed: { name: string; manifest: Manifest; off: boolean; spec: LayerSpec }[] = [];
    for (const name of page?.layers ?? []) {
      const layer = layerByName.get(name);
      const layerSpec = (layer?.spec ?? {}) as LayerSpec;
      const endpoint = layerSpec.sourceEndpointRef
        ? endpointByName.get(layerSpec.sourceEndpointRef)
        : undefined;
      const endpointSpec = (endpoint?.spec ?? {}) as { slug?: string; audience?: string };
      if (!layer || !endpointSpec.slug) {
        refused.push(name);
        continue;
      }
      // A public dashboard may only read from a public Endpoint (UI-19). The Portal refuses
      // the manifest at write time too; the view refuses to ask for the data either way.
      if (spec.visibility === "public" && endpointSpec.audience !== "public") {
        refused.push(name);
        continue;
      }
      // The legend's switch, or the manifest's `visible: false` until the reader touched it.
      const off = hidden ? hidden.includes(name) : layerSpec.visible === false;
      listed.push({ name, manifest: layer, off, spec: layerSpec });
      if (off) {
        continue;
      }
      resolved.push({
        name,
        url: geojsonUrl(endpointSpec.slug, layerSpec, bbox),
        style: STYLES.find((style) => style === layerSpec.style) ?? "circle",
        colorBy: layerSpec.colorBy,
        sizeBy: layerSpec.sizeBy,
        popupProperties: layerSpec.popupProperties,
      });
    }
    // A widget reads through an Endpoint exactly as a layer does, UI-19 included.
    const drawnWidgets: { widget: WidgetSpec; slug?: string }[] = [];
    for (const widget of page?.widgets ?? []) {
      const endpoint = widget.endpointRef ? endpointByName.get(widget.endpointRef) : undefined;
      const endpointSpec = (endpoint?.spec ?? {}) as { slug?: string; audience?: string };
      if (spec.visibility === "public" && endpointSpec.audience !== "public") {
        refused.push(widget.endpointRef ?? widget.widgetType);
        continue;
      }
      drawnWidgets.push({ widget, slug: endpointSpec.slug });
    }
    return { mapLayers: resolved, blocked: refused, legend: listed, widgets: drawnWidgets };
  }, [dashboard, layerManifests, endpointManifests, hidden, bbox, pageIndex]);

  function toggle(name: string, on: boolean) {
    const current = hidden ?? legend.filter((entry) => entry.off).map((entry) => entry.name);
    setHidden(on ? current.filter((n) => n !== name) : [...current, name]);
  }

  const features = useQuery({
    queryKey: ["layer-features", mapLayers.map((layer) => layer.url)],
    queryFn: () => Promise.all(mapLayers.map((layer) => fetchFeatures(layer.url))),
    enabled: mapLayers.length > 0,
    // A dashboard is opened, read and left. Refetching a 60 000-feature layer whenever the
    // window regains focus is not what the reader came for.
    staleTime: 60_000,
    retry: false,
    // A pan moves the bbox and with it the key: the last features stay on the map while the next
    // arrive, so the map is never swapped for a placeholder and the viewport stays (F-UI3-017).
    placeholderData: keepPreviousData,
  });

  const { native, dense } = useMemo(() => {
    const drawn: MapLayer[] = [];
    const overlaid: DenseLayer[] = [];
    mapLayers.forEach((layer, index) => {
      const fetched = features.data?.[index] ?? null;
      if (fetched && rendersWithDeckGl(layer.style, fetched.length)) {
        overlaid.push({ ...layer, features: fetched });
        return;
      }
      // Whatever is left is MapLibre's, and MapLibre knows three styles: an aggregation
      // that never reached the threshold draws as the points it aggregates.
      const style = layer.style === "line" || layer.style === "fill" ? layer.style : "circle";
      // A bare array is not GeoJSON: MapLibre takes a URL, a Feature or a collection.
      const data = fetched ? { type: "FeatureCollection", features: fetched } : undefined;
      drawn.push({ ...layer, style, data });
    });
    return { native: drawn, dense: overlaid };
  }, [mapLayers, features.data]);

  if (dashboards.isPending) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("dashboards.title")} description={t("dashboards.lead")} />
        <Card flush className="overflow-hidden">
          <MapPlaceholder label={t("app.loading")} />
        </Card>
      </div>
    );
  }

  if (dashboards.isError) {
    const message =
      dashboards.error instanceof ApiError
        ? (dashboards.error.problem?.detail ?? dashboards.error.message)
        : t("app.error.generic");
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("dashboards.title")} description={t("dashboards.lead")} />
        <Alert
          role="alert"
          tone="danger"
          actions={
            <Button
              size="sm"
              icon={<Icon name="refresh" className="size-4" />}
              onClick={() => {
                void dashboards.refetch();
              }}
            >
              {t("app.error.retry")}
            </Button>
          }
        >
          {message}
        </Alert>
      </div>
    );
  }

  const editors = (
    <>
      <DashboardEditor
        project={project}
        editing={editingDashboard}
        isNew={isNew}
        openedAs={openedAs}
        onEditingChange={setEditingDashboard}
        onChange={setChange}
        layers={[...layerManifests, ...draftedLayers].map((item) => item.metadata.name)}
        draftedLayers={draftedLayers}
        loadingDrafts={drafts.some((query) => query.isPending)}
      />
      <LayerEditor
        project={project}
        editing={editingLayer}
        isNew={isNew}
        openedAs={openedAs}
        onEditingChange={setEditingLayer}
        onChange={setChange}
        endpoints={endpointManifests}
        spaces={spaceManifests}
        models={modelManifests}
      />
    </>
  );
  const newButtons = (
    <>
      <PermissionGuard project={project} kind="Dashboard" verb="propose">
        <Button
          size="sm"
          variant="primary"
          icon={<Icon name="plus" className="size-4" />}
          onClick={() => {
            setIsNew(true);
            setOpenedAs(undefined);
            setEditingDashboard(dashboardFromManifest(undefined));
          }}
        >
          {t("dashboards.add")}
        </Button>
      </PermissionGuard>
      <PermissionGuard project={project} kind="Layer" verb="propose">
        <Button
          size="sm"
          icon={<Icon name="plus" className="size-4" />}
          onClick={() => {
            setIsNew(true);
            setOpenedAs(undefined);
            setEditingLayer(layerFromManifest(undefined));
          }}
        >
          {t("dashboards.addLayer")}
        </Button>
      </PermissionGuard>
    </>
  );

  if (all.length === 0 || !dashboard) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader
          title={t("dashboards.title")}
          description={t("dashboards.lead")}
          aside={<div className="flex flex-wrap gap-2">{newButtons}</div>}
        />
        {change ? <ChangeNotice change={change} project={project} /> : null}
        {/* The same controls as the header, where an empty page puts the eye (T-1381). */}
        <EmptyState
          icon="dashboards"
          title={t("dashboards.empty")}
              description={t("dashboards.emptyHint")}
          action={<div className="flex flex-wrap justify-center gap-2">{newButtons}</div>}
        />
        {editors}
      </div>
    );
  }

  const spec = dashboard.spec as DashboardSpec;
  const title = localized(spec.title, locale, dashboard.metadata.name);

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        title={title}
        description={t("dashboards.lead")}
        aside={
          <div className="flex flex-wrap items-center gap-2">
            {all.length > 1 ? (
              <label htmlFor={selectId} className="flex items-center gap-2 text-body text-fg-muted">
                <span className="whitespace-nowrap">{t("dashboards.title")}</span>
                <Select
                  id={selectId}
                  value={dashboard.metadata.name}
                  onChange={(event) => {
                    setSelected(event.target.value);
                    setHidden(null);
                    setPageIndex(0);
                  }}
                  className="min-w-56"
                >
                  {all.map((item: Manifest) => (
                    <option key={item.metadata.name} value={item.metadata.name}>
                      {localized((item.spec as DashboardSpec).title, locale, item.metadata.name)}
                    </option>
                  ))}
                </Select>
              </label>
            ) : null}
            {/* This dashboard's own four — edit, save as, work on a copy, delete — behind one
                menu, so the header carries the chooser and what makes a new one, and not five
                controls in a row (T-2288, UI-26). */}
            <ResourceRowActions
              project={project}
              target={{
                project,
                kind: "Dashboard",
                plural: "dashboards",
                name: dashboard.metadata.name,
                label: title,
              }}
              onEdit={() => {
                setIsNew(false);
                setOpenedAs(dashboard.metadata.name);
                setEditingDashboard(dashboardFromManifest(dashboard));
              }}
            />
            {newButtons}
          </div>
        }
      />

      {change ? <ChangeNotice change={change} project={project} /> : null}

      {blocked.length > 0 ? (
        <Alert role="status" tone="warning">
          {t("dashboards.layerBlocked", { layers: blocked.join(", ") })}
        </Alert>
      ) : null}

      {(spec.pages ?? []).length > 1 ? (
        <Tabs
          id={pagesId}
          variant="pill"
          label={t("dashboards.pages")}
          value={String(pageIndex)}
          onChange={(next) => {
            setPageIndex(Number(next));
            setHidden(null);
          }}
          tabs={(spec.pages ?? []).map((page, index) => ({
            value: String(index),
            label: page.title ?? t("dashboards.page", { number: index + 1 }),
          }))}
        />
      ) : null}

      {/* The one page the tabs above select: named by its tab, so a screen reader that lands
          here is told which of them it is reading (UI-16). */}
      <div
        {...((spec.pages ?? []).length > 1 ? tabPanelProps(pagesId, String(pageIndex)) : {})}
        className="flex flex-col gap-section"
      >
      <Card flush className="overflow-hidden">
        <Suspense fallback={<MapPlaceholder label={t("app.loading")} />}>
          {features.isPending && mapLayers.length > 0 ? (
            <MapPlaceholder label={t("app.loading")} />
          ) : dense.length > 0 ? (
            <DeckGlOverlay layers={native} dense={dense} label={title} project={project} />
          ) : (
            <MapLibreView
              layers={native}
              label={title}
              project={project}
              onMoveEnd={onMoveEnd}
            />
          )}
        </Suspense>
      </Card>

      {widgets.length > 0 ? (
        <div aria-label={t("dashboards.widgets")} className="grid gap-3 md:grid-cols-2">
          {widgets.map(({ widget, slug }, index) =>
            widget.widgetType === "grid" && slug && widget.entityType ? (
              // The same grid the explorer renders, configured by the manifest (UI-71, SDK-30):
              // the endpoint and the type come from the widget, so a dashboard cannot point it
              // anywhere else, and the endpoint's Policy still decides every read and write.
              <GridWidget
                key={`${widget.endpointRef}-${widget.entityType}-${index}`}
                project={project}
                slug={slug}
                type={widget.entityType}
                config={widget.grid}
                title={<TypeLink project={project} type={widget.entityType} />}
              />
            ) : widget.widgetType === "temporal-chart" && slug && widget.entityId && widget.property ? (
              <TemporalChart
                key={`${widget.entityId}-${widget.property}-${index}`}
                slug={slug}
                entityId={widget.entityId}
                property={widget.property}
                title={t("dashboards.widget.temporalChart", { property: widget.property })}
              />
            ) : (widget.widgetType === "bar-chart" || widget.widgetType === "histogram") && slug && widget.entityType && widget.property ? (
              <TypeChart
                key={`${widget.widgetType}-${widget.entityType}-${widget.property}-${index}`}
                slug={slug}
                entityType={widget.entityType}
                property={widget.property}
                q={widget.q}
                kind={widget.widgetType}
                title={t(`dashboards.widget.${widget.widgetType === "bar-chart" ? "barTitle" : "histogramTitle"}`, {
                  property: widget.property,
                  type: widget.entityType,
                })}
              />
            ) : (
              <Card padding="sm" key={`${widget.widgetType}-${index}`}>
                <p role="status" className="text-body text-fg-muted">
                  {t("dashboards.widget.unsupported", { type: widget.widgetType })}
                </p>
              </Card>
            ),
          )}
        </div>
      ) : null}

      {legend.length > 0 ? (
        <ul aria-label={t("dashboards.legend")} className="flex flex-wrap gap-2">
          {legend.map((entry) => (
            <Card as="li" flush key={entry.name} className="flex flex-wrap items-center gap-2 px-3 py-2 text-body">
              <Checkbox
                aria-label={`${t("dashboards.show")}: ${entry.name}`}
                checked={!entry.off}
                onChange={(event) => toggle(entry.name, event.target.checked)}
                label={
                  <>
                    <Badge tone="primary" mono>
                      {entry.spec.style ?? "circle"}
                    </Badge>{" "}
                    <span className="font-medium">{entry.name}</span>
                  </>
                }
              />
              {entry.spec.colorBy ? (
                <span className="inline-flex items-center gap-1 text-caption text-fg-muted">
                  <span>{entry.spec.colorBy.domain?.[0] ?? 0}</span>
                  {RAMP.map((stop) => (
                    <span
                      key={stop}
                      aria-hidden="true"
                      className="inline-block size-3 rounded-sm border border-border"
                      style={{ backgroundColor: stop }}
                    />
                  ))}
                  <span>{entry.spec.colorBy.domain?.[1] ?? 100}</span>
                  <span>
                    {t("dashboards.colouredBy", {
                      property: entry.spec.colorBy.property,
                      min: entry.spec.colorBy.domain?.[0] ?? 0,
                      max: entry.spec.colorBy.domain?.[1] ?? 100,
                    })}
                  </span>
                </span>
              ) : null}
              <PermissionGuard project={project} kind="Layer" verb="propose">
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={`${t("dashboards.editLayer")}: ${entry.name}`}
                  onClick={() => {
                    setIsNew(false);
                    setOpenedAs(entry.manifest.metadata.name);
                    setEditingLayer(layerFromManifest(entry.manifest));
                  }}
                >
                  {t("dashboards.edit")}
                </Button>
              </PermissionGuard>
            </Card>
          ))}
        </ul>
      ) : null}
      </div>
      {editors}
    </div>
  );
}
