import { lazy, useState } from "react";
import type { ComponentType, JSX } from "react";
import { RecordLink } from "../components/RecordLink";
import { ManifestGrid } from "../components/grid/ManifestGrid";
import { spaceOf } from "../components/endpoints/sharing";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ApiError, queryKeys } from "../api/client";
import { GRID_PAGE, useKindPages } from "./kindPages";
import i18n from "../i18n";
import { NotFoundState } from "../components/NotFoundState";
import { isChange, localized, ORG_NAMESPACE } from "../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../api/manifest";
import { proposeChecked } from "../api/proposal";
import { ChangeNotice } from "../components/ChangeNotice";
import { ResourceFormDialog } from "../components/ResourceFormDialog";
import { AssistantContextPreview } from "../pages/knowledge/AssistantContextPreview";
import { useCreateFormFromDraft } from "../components/forms/FormRoute";
import { PermissionGuard } from "../components/ui/PermissionGuard";
import { humanizeName } from "../pages/apps/appTitle";
import { ResourceRowActions } from "../components/ResourceRowActions";
import type { EditableForm } from "../components/EditResourceDialog";
import {
  fromMappingManifest,
  mappingSchema,
  mappingUiSchema,
  toMappingManifest,
} from "../schemas/mapping";
import type { MappingForm } from "../schemas/mapping";
import { dataModelSchema, fromDataModelManifest, toDataModelManifest } from "../schemas/datamodel";
import type { DataModelForm } from "../schemas/datamodel";
import { dataAgreementSchema, fromDataAgreementManifest, toDataAgreementManifest } from "../schemas/dataagreement";
import { blueprintSchema, blueprintUiSchema, fromBlueprintManifest, toBlueprintManifest } from "../schemas/blueprint";
import type { BlueprintForm } from "../schemas/blueprint";
import {
  agentProfileSchema,
  agentProfileUiSchema,
  fromAgentProfileManifest,
  toAgentProfileManifest,
} from "../schemas/agentprofile";
import type { AgentProfileForm } from "../schemas/agentprofile";
import { dataOfferSchema, dataOfferUiSchema, fromDataOfferManifest, toDataOfferManifest } from "../schemas/dataoffer";
import type { DataOfferForm } from "../schemas/dataoffer";
import {
  dataSpaceParticipantSchema,
  fromDataSpaceParticipantManifest,
  toDataSpaceParticipantManifest,
} from "../schemas/dataspaceparticipant";
import type { DataSpaceParticipantForm } from "../schemas/dataspaceparticipant";
import { environmentSchema, fromEnvironmentManifest, toEnvironmentManifest } from "../schemas/environment";
import type { EnvironmentForm } from "../schemas/environment";
import { fromModelProjectionManifest, modelProjectionSchema, toModelProjectionManifest } from "../schemas/modelprojection";
import type { ModelProjectionForm } from "../schemas/modelprojection";
import { fromScopeDefinitionManifest, scopeDefinitionSchema, toScopeDefinitionManifest } from "../schemas/scopedefinition";
import type { ScopeDefinitionForm } from "../schemas/scopedefinition";
import type { DataAgreementForm } from "../schemas/dataagreement";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { fromMqttBrokerManifest, mqttBrokerSchema, toMqttBrokerManifest } from "../schemas/mqttbroker";
import type { MqttBrokerForm } from "../schemas/mqttbroker";
import {
  Button,
  EmptyState,
  buttonClass,
  Icon,
  PageHeader,
} from "../components/ui";
// Each section's page loads with its section (T-3280): a list page beside it never pays for a map,
// a graph or an editor it does not draw.
const SpacesPage = lazy(() => import("./SpacesPage").then((module) => ({ default: module.SpacesPage })));
const EndpointsPage = lazy(() => import("./EndpointsPage").then((module) => ({ default: module.EndpointsPage })));
const DashboardsPage = lazy(() => import("./DashboardsPage").then((module) => ({ default: module.DashboardsPage })));
const PipelinesPage = lazy(() => import("./PipelinesPage").then((module) => ({ default: module.PipelinesPage })));
const PoliciesPage = lazy(() => import("./PoliciesPage").then((module) => ({ default: module.PoliciesPage })));
const SubscriptionsPage = lazy(() => import("./SubscriptionsPage").then((module) => ({ default: module.SubscriptionsPage })));
const RegistrationsPage = lazy(() => import("./RegistrationsPage").then((module) => ({ default: module.RegistrationsPage })));
const DataSourcesPage = lazy(() => import("../pages/datasources/DataSourcesPage").then((module) => ({ default: module.DataSourcesPage })));
const AppsCatalog = lazy(() => import("../pages/apps/AppsCatalog").then((module) => ({ default: module.AppsCatalog })));
const MqttBrokersPage = lazy(() => import("../pages/mqttbrokers/MqttBrokersPage").then((module) => ({ default: module.MqttBrokersPage })));
const SyncSourcesPage = lazy(() => import("../pages/sync/SyncSourcesPage").then((module) => ({ default: module.SyncSourcesPage })));
import { assistantDeploymentSchema, assistantDeploymentUiSchema, fromKindManifest, knowledgeSourceSchema, toKindManifest } from "../schemas/knowledge";
import { kindNew } from "../kindWords";

const VIEWS: Record<string, ComponentType<{ project: string; edit?: string }>> = {
  spaces: SpacesPage,
  endpoints: EndpointsPage,
  pipelines: PipelinesPage,
  // A Policy is authored through a form like every other kind, not as YAML (T-2326).
  policies: PoliciesPage,
  // A Subscription is authored through a form too, not as YAML (T-2344).
  subscriptions: SubscriptionsPage,
  // A ContextSourceRegistration too: which space's broker answers with whose data (T-2345, MF-36).
  csrs: RegistrationsPage,
  datasources: DataSourcesPage,
  // The list with the kind's form, then each broker's topic access and who talks to whom (T-3839).
  mqttbrokers: MqttBrokersPage,
  dashboards: DashboardsPage,
  // Apps are a kind, but a card catalogue with a preview frame and a publication action, not
  // a manifest table (AP-18, AP-19, AP-20).
  apps: AppsCatalog,
  // A SyncSource is a running loop as well as a manifest, so its view carries the phase, the
  // revision it carries and the three buttons of MF-30.
  syncsources: SyncSourcesPage,
};

/**
 * The edit form of a kind that has no page of its own. It writes onto the manifest the edit dialog
 * read, so it keeps what it does not show (T-2354). A kind not listed here opens as YAML.
 */
const EDIT_FORMS: Record<string, (t: (key: string) => string, project: string) => EditableForm> = {
  mqttbrokers: (t, project) => ({
    schema: mqttBrokerSchema(t, i18n.resolvedLanguage ?? i18n.language),
    fromManifest: (manifest) => fromMqttBrokerManifest(manifest) as Record<string, unknown>,
    toManifest: (form, stored) => toMqttBrokerManifest(project, form as MqttBrokerForm, stored),
  }),
  mappings: (t) => ({
    schema: mappingSchema(t),
    uiSchema: mappingUiSchema,
    fromManifest: (manifest) => fromMappingManifest(manifest) as Record<string, unknown>,
    toManifest: (form, stored) => toMappingManifest(stored, form as MappingForm),
  }),
  datamodels: (t) => ({
    schema: dataModelSchema(t),
    fromManifest: (manifest) => fromDataModelManifest(manifest) as Record<string, unknown>,
    toManifest: (form, stored) => toDataModelManifest(stored, form as DataModelForm),
  }),
  dataagreements: (t, project) => ({
    schema: dataAgreementSchema(t),
    fromManifest: (manifest) => fromDataAgreementManifest(manifest) as Record<string, unknown>,
    toManifest: (form, stored) => toDataAgreementManifest(project, form as DataAgreementForm, stored),
  }),
  dataoffers: (t, project) => ({
    schema: dataOfferSchema(t),
    uiSchema: dataOfferUiSchema,
    fromManifest: (manifest) => fromDataOfferManifest(manifest) as Record<string, unknown>,
    toManifest: (form, stored) => toDataOfferManifest(project, form as DataOfferForm, stored),
  }),
  projections: (t, project) => ({
    schema: modelProjectionSchema(t),
    fromManifest: (manifest) => fromModelProjectionManifest(manifest) as Record<string, unknown>,
    toManifest: (form, stored) => toModelProjectionManifest(project, form as ModelProjectionForm, stored),
  }),
  scopedefinitions: (t, project) => ({
    schema: scopeDefinitionSchema(t),
    fromManifest: (manifest) => fromScopeDefinitionManifest(manifest) as Record<string, unknown>,
    toManifest: (form, stored) => toScopeDefinitionManifest(project, form as ScopeDefinitionForm, stored),
  }),
  blueprints: (t) => ({
    schema: blueprintSchema(t),
    uiSchema: blueprintUiSchema,
    fromManifest: (manifest) => fromBlueprintManifest(manifest) as Record<string, unknown>,
    toManifest: (form, stored) => toBlueprintManifest(form as BlueprintForm, stored),
  }),
  dataspaceparticipants: (t) => ({
    schema: dataSpaceParticipantSchema(t),
    fromManifest: (manifest) => fromDataSpaceParticipantManifest(manifest) as Record<string, unknown>,
    toManifest: (form, stored) => toDataSpaceParticipantManifest(form as DataSpaceParticipantForm, stored),
  }),
  environments: (t) => ({
    schema: environmentSchema(t),
    fromManifest: (manifest) => fromEnvironmentManifest(manifest) as Record<string, unknown>,
    toManifest: (form, stored) => toEnvironmentManifest(form as EnvironmentForm, stored),
  }),
  agentprofiles: (t) => ({
    schema: agentProfileSchema(t),
    uiSchema: agentProfileUiSchema,
    fromManifest: (manifest) => fromAgentProfileManifest(manifest) as Record<string, unknown>,
    toManifest: (form, stored) => toAgentProfileManifest(form as AgentProfileForm, stored),
  }),
  // The knowledge assistant's kinds (T-3057): created and edited from their lists, which the
  // Knowledge sources page links; their references are pickers (pickers.ts).
  knowledgesources: (t, project) => ({
    schema: knowledgeSourceSchema(t),
    fromManifest: (manifest) => fromKindManifest(manifest),
    toManifest: (form, stored) => toKindManifest("KnowledgeSource", project, form, stored),
  }),
  assistantdeployments: (t, project) => ({
    schema: assistantDeploymentSchema(t),
    uiSchema: assistantDeploymentUiSchema,
    fromManifest: (manifest) => fromKindManifest(manifest),
    toManifest: (form, stored) => toKindManifest("AssistantDeployment", project, form, stored),
    // What the deployment being edited will read, before it is proposed (AG-121, T-4693).
    afterFields: (manifest) => <AssistantContextPreview project={project} manifest={manifest as Manifest} />,
  }),
};


/**
 * The kinds a person creates from their list with the form above, by the kind's name (T-1542). A
 * Mapping is created on the Models page's Mappings view, which writes its transformation and golden
 * tests; a DataModel in the LinkML editor. Neither is offered here.
 */
const CREATE_KINDS: Record<string, string> = {
  mqttbrokers: "MqttBroker",
  dataagreements: "DataAgreement",
  dataoffers: "DataOffer",
  projections: "ModelProjection",
  scopedefinitions: "ScopeDefinition",
  blueprints: "Blueprint",
  agentprofiles: "AgentProfile",
  dataspaceparticipants: "DataSpaceParticipant",
  environments: "Environment",
  knowledgesources: "KnowledgeSource",
  assistantdeployments: "AssistantDeployment",
};

/** Whether `plural` is a kind this page lists, so `/{plural}/{name}/edit` is a page (UI-91). */
export function hasEditForm(plural: string): boolean {
  return Object.hasOwn(VIEWS, plural) || Object.hasOwn(EDIT_FORMS, plural) || Object.hasOwn(CREATE_KINDS, plural);
}

/**
 * Kinds jc-core keeps one of per organization, at one path whatever the name (T-1544): New is offered
 * only once the list has answered that there is none, so a second one is never proposed over the first.
 */
const ONE_PER_ORGANIZATION = new Set(["dataspaceparticipants"]);

/** `/api/v1/projects/{project}/{plural}`: a kind's own page, or its resources in a table (MF-11…MF-15). */
export function ResourceListPage({
  project,
  plural,
  edit,
}: {
  project: string;
  plural: string;
  /** `?edit=<name>`: open this kind's own editor on that resource at once (T-2281). */
  edit?: string;
}): JSX.Element {
  const View = VIEWS[plural];
  if (View) {
    return <View project={project} edit={edit} />;
  }
  return <KindList project={project} plural={plural} />;
}

/**
 * The resources of one kind in a table, with the kind's form to create and edit them where it has
 * one. A page of its own for a project's kinds; `embedded` in a tab of another page, such as the
 * Organization's, where the page already carries the heading of level one.
 */
export function KindList({
  project,
  plural,
  embedded = false,
  createRefused,
}: {
  project: string;
  plural: string;
  embedded?: boolean;
  /**
   * Why a new one would be refused before the form is filled, such as an allowance used up
   * (T-5589): New stays visible, closed, with this as its description, and the empty list says it.
   * The API's own refusal stays the gate.
   */
  createRefused?: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const queryClient = useQueryClient();
  const form = EDIT_FORMS[plural]?.(t, project);
  const createKind = form ? CREATE_KINDS[plural] : undefined;
  // The create form is a page at `/{plural}/new` on a routed list (T-2474).
  const [dialogOpen, setDialogOpen, handedDraft] = useCreateFormFromDraft();
  const [values, setValues] = useState<Record<string, unknown> | undefined>(undefined);
  const [formError, setFormError] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const create = useMutation({
    mutationFn: (next: Record<string, unknown>) => {
      setFormError(null);
      return proposeChecked(project, plural, form?.toManifest(next, undefined) as ResourceProposal, true);
    },
    onSuccess: (result) => {
      if (isChange(result)) setChange(result);
      setDialogOpen(false);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, plural) });
    },
    onError: (err) =>
      setFormError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      ),
  });
  const { list, items, server } = useKindPages(project, plural);

  // The API is what knows the project's kinds: a plural it does not serve is an address with no
  // page, not a list that failed and could be tried again (T-2749, UI-15).
  if (list.error instanceof ApiError && list.error.status === 404) {
    return <NotFoundState />;
  }
  // A kind with no page of its own still opens under a heading that names it, in the person's
  // own language where the navigation already has a word for it, and read as words where it
  // does not — never the bare URL segment the table used as its caption (UI-01, UI-16).
  const title = t(`nav.${plural}`, { defaultValue: humanizeName(plural) });
  // The organization's own kinds are the organization's, not a project's (T-4831).
  const holder = project === ORG_NAMESPACE ? "Organization" : "";
  const newButton =
    createKind && form && !(ONE_PER_ORGANIZATION.has(plural) && (list.data === undefined || items.length > 0)) ? (
      <PermissionGuard project={project} kind={createKind} verb="propose">
        <Button
          variant="primary"
          data-shortcut="new"
          icon={<Icon name="plus" className="size-4" />}
          disabled={createRefused !== undefined}
          disabledReason={createRefused}
          onClick={() => {
            setFormError(null);
            setValues(undefined);
            setDialogOpen(true);
          }}
        >
          {kindNew(createKind, t)}
        </Button>
      </PermissionGuard>
    ) : undefined;
  return (
    <div className="flex flex-col gap-section">
      {embedded ? (
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h2 className="text-title font-semibold text-fg">{title}</h2>
            <p className="text-body text-fg-muted">{t(`resourceList.lead${holder}`, { kind: title })}</p>
          </div>
          {newButton}
        </div>
      ) : (
        <PageHeader compact title={title} description={t(`resourceList.lead${holder}`, { kind: title })} actions={newButton} />
      )}
      {change ? <ChangeNotice change={change} project={project} /> : null}
      <ManifestGrid<Manifest>
        caption={title}
        storageKey={`kind.${plural}`}
        searchLabel={t("grid.searchIn", { list: title })}
        rows={items}
        rowKey={(item) => item.metadata.name}
        query={list}
        pageSize={GRID_PAGE}
        server={server}
        name={{
          id: "name",
          label: t("resourceList.name"),
          text: (item) => localized(item.metadata.title, locale, item.metadata.name),
          cell: (item) => (
            <div className="min-w-0">
              <RecordLink project={project} plural={plural} name={item.metadata.name}>
                {localized(item.metadata.title, locale, item.metadata.name)}
              </RecordLink>
              {item.metadata.title ? (
                <div className="mt-0.5 truncate font-mono text-caption text-fg-subtle">{item.metadata.name}</div>
              ) : null}
            </div>
          ),
        }}
        columns={[
          ...(items.some((item) => spaceOf(item))
            ? [{ id: "space", label: t("grid.field.space"), filter: true, text: (item: Manifest) => spaceOf(item) ?? "" }]
            : []),
          {
            id: "phase",
            label: t("resourceList.phase"),
            filter: true,
            text: (item) => item.status?.phase ?? "",
            cell: (item) => <LifecycleBadge kind="phase" value={item.status?.phase} />,
          },
        ]}
        // One menu at the end of the row, for every kind that falls through here (T-2287).
        actions={(item) => (
          <ResourceRowActions
            project={project}
            target={{ project, kind: item.kind, plural, name: item.metadata.name, label: localized(item.metadata.title, locale, item.metadata.name) }}
            form={form}
          />
        )}
      empty={<EmptyState bare
            title={t("resourceList.empty")}
            // A page with no create action names none in its hint (T-2488).
            description={
              createRefused ??
              (newButton && createKind
                ? t(`resourceList.emptyHintCreate${holder}`, { new: kindNew(createKind, t) })
                : t(`resourceList.emptyHintFor.${plural}`, { defaultValue: t(`resourceList.emptyHint${holder}`) }))
            }
            action={
              createRefused !== undefined ? undefined :
              newButton ??
              // The hint sends a person to the assistant; the button takes them there. A kind
              // whose hint says it is changed elsewhere offers nothing here (T-3246).
              (i18n.exists(`resourceList.emptyHintFor.${plural}`) ? undefined : (
                <Link to="/projects/$project/assistant" params={{ project }} className={buttonClass("secondary", "md")}>
                  {t("resourceList.askAssistant")}
                </Link>
              ))
            } />}
      />
      {createKind && form ? (
        <ResourceFormDialog<Record<string, unknown>>
          kind={createKind}
          open={dialogOpen}
          onOpenChange={setDialogOpen}
          title={kindNew(createKind, t)}
          description={t("resourceList.newHint", { kind: createKind })}
          schema={form.schema}
          uiSchema={form.uiSchema}
          formData={values}
          onChange={setValues}
          project={project}
          draftKind={createKind}
          draftName={handedDraft}
          plural={plural}
          source={{
            toManifest: (next) => form.toManifest(next, undefined),
            fromManifest: (manifest) => form.fromManifest(manifest),
          }}
          submitLabel={t("resourceList.propose")}
          afterFields={form.afterFields?.(form.toManifest(values ?? {}, undefined))}
          submitting={create.isPending}
          error={formError}
          onSubmit={(next) => create.mutate(next)}
        />
      ) : null}
    </div>
  );
}
