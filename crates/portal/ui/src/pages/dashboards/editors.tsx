import { useMemo } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { plainTitle, prune, refName } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { EntityFilters } from "../../components/entities/EntityFilters";
import { Alert } from "../../components/ui";
import { filterSlotsOf, useModelSource } from "../../components/entities/filters";
import { useProposal } from "../../api/proposal";
import { ResourceFormDialog } from "../../components/ResourceFormDialog";
import { dashboardSchema, dashboardUiSchema, layerSchema, layerUiSchema } from "../../schemas/kinds";
import { entityTypesOf, spaceOf } from "../spaces/spaceFacts";

const API_VERSION = "joinedcontext.com/v1alpha1";

export interface DashboardForm {
  name: string;
  title: string;
  visibility: string;
  pages: {
    title?: string;
    layout?: string;
    layers?: string[];
    widgets?: {
      widgetType: string;
      endpointRef?: string;
      entityId?: string;
      property?: string;
      /** A `grid` widget: the type it shows and the grid's own configuration (T-1440). */
      entityType?: string;
      grid?: Record<string, unknown>;
    }[];
  }[];
}

export interface LayerForm {
  name: string;
  sourceEndpointRef: string;
  entityType: string;
  style: string;
  visible?: boolean;
  filter?: { q?: string; scopeQ?: string; geoQ?: string };
  colorBy?: { property?: string; palette?: string; domain?: number[] };
  sizeBy?: { property?: string; range?: number[] };
  popupProperties?: string[];
}

function envelope(
  project: string,
  kind: string,
  name: string,
  spec: Record<string, unknown>,
) {
  return { apiVersion: API_VERSION, kind, metadata: { name, namespace: project }, spec: prune(spec) };
}

export function dashboardToManifest(project: string, form: DashboardForm) {
  const { name, ...spec } = form;
  return envelope(project, "Dashboard", name, spec);
}

export function dashboardFromManifest(document: unknown): DashboardForm {
  const manifest = (document ?? {}) as Partial<Manifest>;
  const spec = (manifest.spec ?? {}) as Omit<DashboardForm, "name">;
  return {
    name: manifest.metadata?.name ?? "",
    title: plainTitle(spec.title) ?? "",
    visibility: spec.visibility ?? "project",
    pages: spec.pages ?? [{ layout: "full-map", layers: [] }],
  };
}

export function layerToManifest(project: string, form: LayerForm) {
  const { name, visible, ...spec } = form;
  // `true` is the default and is not written; `false` is the whole point of the member.
  return envelope(project, "Layer", name, { ...spec, ...(visible === false ? { visible } : {}) });
}

export function layerFromManifest(document: unknown): LayerForm {
  const manifest = (document ?? {}) as Partial<Manifest>;
  const spec = (manifest.spec ?? {}) as Omit<LayerForm, "name">;
  return {
    name: manifest.metadata?.name ?? "",
    sourceEndpointRef: spec.sourceEndpointRef ?? "",
    entityType: spec.entityType ?? "",
    style: spec.style ?? "circle",
    visible: spec.visible ?? true,
    filter: spec.filter,
    colorBy: spec.colorBy,
    sizeBy: spec.sizeBy,
    popupProperties: spec.popupProperties,
  };
}

export interface EditorProps<T> {
  project: string;
  /** The form being edited; `null` keeps the dialog closed. */
  editing: T | null;
  isNew: boolean;
  /**
   * The name the resource already carries, as the page opened it — not the form's own, which a
   * rename would have changed. A renamed manifest is a second resource at a new path (MF-11).
   */
  openedAs?: string;
  onEditingChange: (form: T | null) => void;
  onChange: (change: Change) => void;
}

export function DashboardEditor({
  project,
  editing,
  isNew,
  openedAs,
  onEditingChange,
  onChange,
  layers,
  draftedLayers = [],
  loadingDrafts = false,
}: EditorProps<DashboardForm> & {
  layers: string[];
  /** Layers drafted with the dashboard, proposed with it; not proposable until they are read. */
  draftedLayers?: Manifest[];
  loadingDrafts?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const proposal = useProposal(project, "dashboards", (change) => {
    onChange(change);
    onEditingChange(null);
  });
  const schema = useMemo(() => dashboardSchema(t, layers), [t, layers]);
  return (
    <ResourceFormDialog<DashboardForm>
      open={editing !== null}
      onOpenChange={(open) => {
        if (!open) {
          onEditingChange(null);
          proposal.reset();
        }
      }}
      title={isNew ? t("dashboards.add") : t("dashboards.edit")}
      description={t("dashboards.addHint")}
      schema={schema}
      uiSchema={dashboardUiSchema}
      lockedName={isNew ? undefined : openedAs}
      project={project}
      // The form edits a Portal draft, so a second window and the assistant see the same text
      // (AG-61, UI-47). The proposal is the manifest itself: `propose_draft` has no dashboards.
      draftKind="Dashboard"
      plural="dashboards"
      draftName={openedAs ?? (editing?.name || undefined)}
      formData={editing ?? undefined}
      submitLabel={t("dashboards.propose")}
      disabled={loadingDrafts}
      submitting={proposal.mutation.isPending}
      error={proposal.error}
      source={{
        toManifest: (form) => dashboardToManifest(project, form),
        fromManifest: dashboardFromManifest,
      }}
      onSubmit={(form) => {
        const drawn = new Set(form.pages.flatMap((page) => page.layers ?? []));
        proposal.mutation.mutate({
          body: dashboardToManifest(project, form),
          create: isNew,
          bundle: draftedLayers.filter((layer) => drawn.has(layer.metadata.name)),
        });
      }}
      onChange={(form) => {
        if (form) {
          onEditingChange(form);
        }
      }}
    />
  );
}

export function LayerEditor({
  project,
  editing,
  isNew,
  openedAs,
  onEditingChange,
  onChange,
  endpoints,
  spaces,
  models,
}: EditorProps<LayerForm> & { endpoints: Manifest[]; spaces: Manifest[]; models: Manifest[] }): JSX.Element {
  const { t } = useTranslation();
  const proposal = useProposal(project, "layers", (change) => {
    onChange(change);
    onEditingChange(null);
  });
  // The type and the filter come from the space the chosen endpoint publishes (UI-33): its
  // DataModel gives the types and the typed attributes the filter rows offer.
  const endpoint = endpoints.find((e) => e.metadata.name === editing?.sourceEndpointRef);
  const space = endpoint ? spaces.find((s) => s.metadata.name === spaceOf(endpoint)) : undefined;
  const model = models.find((m) => m.metadata.name === refName(space?.spec.dataModelRef));
  const types = useMemo(() => (model ? entityTypesOf(model) : []), [model]);
  const slots = filterSlotsOf(useModelSource(project, model), editing?.entityType || undefined);
  const endpointNames = useMemo(() => endpoints.map((e) => e.metadata.name), [endpoints]);
  const schema = useMemo(() => layerSchema(t, endpointNames, types), [t, endpointNames, types]);

  return (
    <ResourceFormDialog<LayerForm>
      open={editing !== null}
      onOpenChange={(open) => {
        if (!open) {
          onEditingChange(null);
          proposal.reset();
        }
      }}
      title={isNew ? t("dashboards.addLayer") : t("dashboards.editLayer")}
      description={t("dashboards.layerHint")}
      schema={schema}
      uiSchema={layerUiSchema}
      lockedName={isNew ? undefined : openedAs}
      project={project}
      draftKind="Layer"
      plural="layers"
      draftName={openedAs ?? (editing?.name || undefined)}
      formData={editing ?? undefined}
      submitLabel={t("dashboards.propose")}
      submitting={proposal.mutation.isPending}
      error={proposal.error}
      source={{
        toManifest: (form) => layerToManifest(project, form),
        fromManifest: layerFromManifest,
      }}
      onSubmit={(form) => proposal.mutation.mutate({ body: layerToManifest(project, form), create: isNew })}
      onChange={(form) => {
        if (form) {
          onEditingChange(form);
        }
      }}
    >
      {/* A layer whose endpoint is gone shows the type and filter rows of nothing, which read as
          a layer with no filters rather than as a layer nobody can finish. The list is only
          judged once it has arrived with entries in it, so a project still loading — or one with
          no endpoint at all, where the select above says as much — is never told this. */}
      {editing && !endpoint && editing.sourceEndpointRef && endpoints.length > 0 ? (
        <Alert tone="warning">
          {t("dashboards.layerEndpointGone", { name: editing.sourceEndpointRef })}
        </Alert>
      ) : null}
      {editing && endpoint ? (
        <EntityFilters
          id="layer"
          types={types}
          space={space?.metadata.name}
          slots={slots}
          value={{
            type: editing.entityType || undefined,
            q: editing.filter?.q,
            scopeQ: editing.filter?.scopeQ,
            attrs: editing.popupProperties,
          }}
          onChange={(query) =>
            onEditingChange({
              ...editing,
              entityType: query.type ?? "",
              filter: prune({ ...editing.filter, q: query.q, scopeQ: query.scopeQ }),
              popupProperties: query.attrs,
            })
          }
        />
      ) : null}
    </ResourceFormDialog>
  );
}
