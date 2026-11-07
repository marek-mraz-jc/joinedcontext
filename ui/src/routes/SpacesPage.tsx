import { useCreateForm } from "../components/forms/FormRoute";
import { RecordLink } from "../components/RecordLink";
import { useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { PermissionGuard } from "../components/ui/PermissionGuard";
import { Skeleton } from "../components/ui/Skeleton";
import type { JSX } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap, whilePending } from "../api/client";
import { proposeChecked } from "../api/proposal";
import { exactBytes, sizeText, spaceUsageQuery, usageRefusal } from "../api/spaceUsage";
import type { SpaceUsage } from "../api/spaceUsage";
import { asManifests, isChange, localized, refName, rewritten, storedMetadata } from "../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../api/manifest";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ResourceFormDialog } from "../components/ResourceFormDialog";
import { ManifestGrid } from "../components/grid/ManifestGrid";
import { ChangeNotice } from "../components/ChangeNotice";
import { ProjectQuota, useProjectUsage } from "../components/ProjectQuota";
import type { ResourceTarget } from "../components/DeleteResourceDialog";
import { ResourceRowActions } from "../components/ResourceRowActions";
import { contextSpaceSchema } from "../schemas/kinds";
import { UsageInfo } from "../components/SpaceUsageInfo";
import { modelsOfSpace } from "../pages/spaces/spaceFacts";
import {
  Alert,
  Badge,
  Button,
  EmptyState,
  Icon,
  PageHeader,
} from "../components/ui";

export interface SpaceForm {
  name: string;
  title?: string;
  dataModelRef?: string;
  defaultLocale?: string;
  isSandbox?: boolean;
  ttlDays?: number;
  /** Pins the `{space}` segment of a space that predates PF-84; kept as it is on edit. */
  urnSegment?: string;
}

/** The `{space}` segment the space's entity ids carry: its pin, else `{project}-{name}` (PF-84). */
export function spaceSegment(project: string, name: string, pin?: string): string {
  return pin ?? `${project}-${name}`;
}

/**
 * The form as the manifest. On an edit, `stored` is the manifest it started from: its description
 * and labels travel on, and a title box left as it was keeps the stored title in every language
 * (T-2470); a title the person changed replaces the language the box showed and keeps the others
 * (UI-50, T-2764), and a cleared one goes.
 */
export function toEnvelope(project: string, form: SpaceForm, stored?: unknown, locale = "en") {
  const { name, title, ...spec } = form;
  const { title: storedTitle, ...kept } = storedMetadata(stored);
  // The model the space already names goes back as the manifest wrote it, `{ kind, name }` or a
  // bare name, so an untouched Save proposes no change to it (T-2872).
  const storedModel = (stored as { spec?: { dataModelRef?: unknown } } | undefined)?.spec?.dataModelRef;
  const model =
    spec.dataModelRef && refName(storedModel) === spec.dataModelRef ? { dataModelRef: storedModel } : {};
  const unchanged =
    storedTitle !== undefined &&
    (title ?? "") === localized(storedTitle as string | Record<string, string>, locale, "");
  const written = unchanged ? storedTitle : title?.trim() ? rewritten(storedTitle, title, locale) : undefined;
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: {
      ...kept,
      name,
      namespace: project,
      ...(written !== undefined ? { title: written } : {}),
    },
    spec: { ...spec, ...model },
  };
}

/** A draft or the YAML view read back into the form: the envelope's name, title and spec. */
export function fromEnvelope(manifest: unknown, locale = "en"): SpaceForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string; title?: unknown }; spec?: object };
  // A manifest's title is a locale map (MF-01), and the form has one box: the box reads the box's
  // language. ponytail: a title in several languages collapses to the one shown if the person
  // edits it, which is what one Title box has always meant here.
  const title = localized(
    envelope.metadata?.title as string | Record<string, string> | undefined,
    locale,
    "",
  );
  const spec = (envelope.spec ?? {}) as { dataModelRef?: unknown };
  // The picker holds a model by its name; the seeds write it `{ kind: DataModel, name }` (T-2872).
  const model = refName(spec.dataModelRef);
  return {
    ...spec,
    ...(model ? { dataModelRef: model } : {}),
    name: envelope.metadata?.name ?? "",
    ...(title === "" ? {} : { title }),
  } as SpaceForm;
}


/** Context Spaces of one project: what exists, what it costs against the quota, where it lives. */
/**
 * What a person can do with one space: Open in the row, the rest behind its menu (T-2279, UI-26).
 *
 * The dialogs are rendered here and not inside the menu, because a menu unmounts when it closes and
 * would take an open dialog with it; each action component is asked for its dialog alone
 * (`trigger={false}`) and the row says when it is open. An action the person's role cannot take is
 * not listed at all, the way the permission guard used to hide its button.
 */
function SpaceRowActions({
  project,
  name,
  target,
  locale,
  sourceUrl,
}: {
  project: string;
  name: string;
  target: ResourceTarget;
  locale: string;
  sourceUrl?: string | null;
}): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  return (
    <ResourceRowActions
      project={project}
      target={target}
      scope={{ kind: "space", name }}
      // Open first, as on every list's menu (T-6095); the name opens it too.
      extra={[
        {
          key: "open",
          label: t("resourceList.open"),
          onSelect: () => void navigate({ to: "/projects/$project/$plural/$name", params: { project, plural: "spaces", name } }),
        },
      ]}
      // The kind's own form, not the manifest as text: the same schema and envelope the create
      // dialog uses (T-2278, UI-61).
      form={{
        schema: contextSpaceSchema(t),
        fromManifest: (manifest) => fromEnvelope(manifest, locale) as unknown as Record<string, unknown>,
        toManifest: (edited, stored) =>
          toEnvelope(project, edited as unknown as SpaceForm, stored, locale),
      }}
      // The row's name opens the space (T-6095): no second "Look inside" button beside the menu.
      sourceUrl={sourceUrl}
    />
  );
}

/** A space's entity count, the reason when there is none, or that it is on its way. */
function EntityCount({
  usage,
  numbers,
}: {
  usage: { data?: { entities: number }; error: unknown; isPending: boolean } | undefined;
  numbers: Intl.NumberFormat;
}): JSX.Element {
  const { t } = useTranslation();
  if (usage?.data !== undefined) {
    return <span data-testid="space-entities">{numbers.format(usage.data.entities)}</span>;
  }
  if (usage === undefined || usage.isPending) {
    return (
      <span role="status" className="inline-block">
        <Skeleton className="ml-auto h-3 w-12" />
        <span className="sr-only">{t("spaces.entitiesCounting")}</span>
      </span>
    );
  }
  const reason =
    usage.error instanceof DOMException && usage.error.name === "TimeoutError"
      ? t("spaces.entitiesSlow")
      : t("spaces.entitiesUnknown", { reason: usageRefusal(usage.error) });
  return (
    <span className="text-fg-subtle" title={reason}>
      <span aria-hidden="true">—</span>
      <span className="sr-only">{reason}</span>
    </span>
  );
}

/** One size under its count, exact in the tooltip; "—" with the reason when the broker gave none
 *  (it leaves `bytes` out for a space it could not measure in time, e.g. Helsinki on dev). */
function SizeLine({ bytes, known, locale, label, testId = "space-size" }: { bytes: number | undefined; known: boolean; locale: string; label: string; testId?: string }): JSX.Element | null {
  const { t } = useTranslation();
  if (!known) return null;
  if (bytes === undefined) {
    return (
      <div data-testid={testId} className="text-caption text-fg-subtle" title={t("spaces.sizeMissing")}>
        <span aria-hidden="true">— </span>
        <span className="sr-only">{t("spaces.sizeMissing")}</span>
      </div>
    );
  }
  return (
    <div data-testid={testId} className="text-caption text-fg-muted" title={`${label}: ${exactBytes(bytes, locale)}`}>
      {sizeText(bytes, locale)}
    </div>
  );
}

/** The space's history: its temporal records ("≈" when the broker estimated them) and their size. */
function HistoryCount({ usage, numbers, locale }: { usage: SpaceUsage | undefined; numbers: Intl.NumberFormat; locale: string }): JSX.Element | null {
  const { t } = useTranslation();
  if (!usage) return null;
  const records = usage.attrInstances;
  return (
    <>
      <span data-testid="space-history" title={usage.estimated ? t("spaces.historyEstimated") : undefined}>
        {records === undefined || records === null ? "—" : `${usage.estimated ? "≈ " : ""}${numbers.format(records)}`}
      </span>
      <SizeLine bytes={usage.bytes?.attrInstances ?? (records === 0 ? 0 : undefined)} known locale={locale} label={t("spaces.field.historySize")} testId="space-history-size" />
    </>
  );
}

export function SpacesPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";

  const usage = useProjectUsage(project);
  // The assistant may have sent the person here with a new space already drafted (`?draft=`,
  // AG-45, UI-45): the dialog opens on it and the form loads the draft by its name, the way the
  // data sources and pipelines pages do.
  const [urlDraftName] = useState(() =>
    typeof window === "undefined"
      ? undefined
      : (new URLSearchParams(window.location.search).get("draft") ?? undefined),
  );
  // The create form is a page at `/spaces/new` on a routed list (T-2474); a drafted space opens it
  // where the person landed.
  const [routedOpen, setRoutedOpen] = useCreateForm();
  const [fromDraft, setFromDraft] = useState(urlDraftName !== undefined);
  const dialogOpen = routedOpen || fromDraft;
  const setDialogOpen = (open: boolean) => {
    if (!open) {
      setFromDraft(false);
    }
    setRoutedOpen(open);
  };
  // The dialog is controlled: its draft, its YAML view and its check all read what it holds.
  const [form, setForm] = useState<SpaceForm | undefined>(undefined);
  const [change, setChange] = useState<Change | null>(null);
  const [formError, setFormError] = useState<string | null>(null);

  const list = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    // A change on its way polls until it lands (T-1392).
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "spaces" } },
        }),
      ),
  });

  const models = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "datamodels" } },
        }),
      ),
  });

  const create = useMutation({
    mutationFn: async (form: SpaceForm) => {
      setFormError(null);
      return proposeChecked(
        project,
        "spaces",
        toEnvelope(project, form) as ResourceProposal,
        true,
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setDialogOpen(false);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "spaces") });
    },
    onError: (err) => {
      setFormError(
        err instanceof ApiError
          ? (err.problem?.detail ?? err.message)
          : err instanceof Error
            ? err.message
            : t("app.error.generic"),
      );
    },
  });

  const listed = asManifests(list.data?.items ?? []);
  const usages = useQueries({
    queries: listed.map((space) => spaceUsageQuery(project, space.metadata.name)),
  });
  const spaces = listed;
  const usageOf = (name: string) => usages[listed.findIndex((space) => space.metadata.name === name)];
  const numbers = new Intl.NumberFormat(locale);
  // The pointer names a model even while the list is on its way or cannot be read; the models that
  // name the space follow it, as on the space's own page (T-2760).
  const modelOf = (space: Manifest): string => {
    const pointer = refName((space.spec as { dataModelRef?: unknown }).dataModelRef);
    return [
      ...new Set([
        ...(pointer ? [pointer] : []),
        ...modelsOfSpace(space, asManifests(models.data?.items ?? [])).map((m) => m.metadata.name),
      ]),
    ].join(", ");
  };
  // The API counts what the project holds and knows which quota is in force, the project's own
  // or the organization's default (PF-73, PF-75); the page only reads the numbers.
  const contextSpaces = usage.data?.contextSpaces;
  const limit = contextSpaces?.limit;
  const quotaExceeded = limit !== undefined && (contextSpaces?.used ?? spaces.length) >= limit;

  // The same control in the header and in the empty list, disabled with the reason for a viewer.
  const addButton = (
    <PermissionGuard project={project} kind="ContextSpace" verb="propose">
      <Button
        variant="primary"
        disabled={quotaExceeded}
        // On the control, not only in the banner above it (UI-44): a hard-disabled button
        // leaves the tab order, so whoever cannot see that it is greyed out never learns why.
        // A permission the caller lacks wins over this, because the guard clones it last.
        disabledReason={quotaExceeded ? t("quota.exceeded", { limit }) : undefined}
        icon={<Icon name="plus" className="size-4" />}
        onClick={() => {
          setFormError(null);
          setForm(undefined);
          setDialogOpen(true);
        }}
      >
        {t("spaces.add")}
      </Button>
    </PermissionGuard>
  );
  // The quickest way to a space with data: a file proposes the model, the space, its Endpoint and
  // the person's write grant in one change (T-6450). Nothing else on this list led there.
  const fromFileButton = (
    <PermissionGuard project={project} kind="ContextSpace" verb="propose">
      <Button
        variant="secondary"
        disabled={quotaExceeded}
        disabledReason={quotaExceeded ? t("quota.exceeded", { limit }) : undefined}
        icon={<Icon name="import" className="size-4" />}
        onClick={() => void navigate({ to: "/projects/$project/spaces/upload", params: { project } })}
      >
        {t("spaces.fromFile")}
      </Button>
    </PermissionGuard>
  );
  const addButtons = (
    <>
      {fromFileButton}
      {addButton}
    </>
  );

  return (
    <div className="flex flex-col gap-section">
      <ManifestGrid<Manifest>
        caption={t("spaces.title")}
        storageKey="spaces"
        searchLabel={t("grid.searchIn", { list: t("spaces.title") })}
        header={(toolbar) => (
          <>
            <PageHeader
              compact
              title={t("spaces.title")}
              description={t("spaces.lead")}
              aside={toolbar}
              actions={addButtons}
            />
            {quotaExceeded ? (
              <Alert role="status" tone="warning">
                {t("quota.exceeded", { limit })}
              </Alert>
            ) : null}
            {change ? <ChangeNotice change={change} project={project} /> : null}
          </>
        )}
        rows={spaces}
        rowKey={(space) => space.metadata.name}
        query={list}
        name={{
          id: "name",
          label: t("spaces.field.name"),
          text: (space) => `${localized(space.metadata.title, locale, space.metadata.name)} ${space.metadata.name}`,
          cell: (space) => {
            const spec = space.spec as { isSandbox?: boolean; ttlDays?: number; urnSegment?: string };
            return (
              <div className="min-w-0">
                <RecordLink project={project} plural="spaces" name={space.metadata.name}>
                  {localized(space.metadata.title, locale, space.metadata.name)}
                </RecordLink>
                {/* The entity-id segment is detail, read on hover like a description (T-6095). */}
                <div
                  className="mt-0.5 flex flex-wrap items-center gap-1.5 font-mono text-caption text-fg-subtle"
                  title={t("spaces.segment", { segment: spaceSegment(project, space.metadata.name, spec.urnSegment) })}
                >
                  {space.metadata.title ? <span>{space.metadata.name}</span> : null}
                  {spec.isSandbox ? <Badge tone="warning">{t("spaces.sandbox", { days: spec.ttlDays ?? 0 })}</Badge> : null}
                </div>
              </div>
            );
          },
        }}
        columns={[
          {
            id: "model",
            label: t("spaces.field.dataModel"),
            filter: true,
            text: modelOf,
            // Each model one click away, as the models list links each space (not a mono id).
            cell: (space) =>
              modelOf(space) ? (
                <span className="flex flex-col gap-0.5">
                  {modelOf(space).split(", ").map((one) => (
                    <Link
                      key={one}
                      to="/projects/$project/models/$name"
                      params={{ project, name: one }}
                      className="focus-ring w-fit text-primary-soft-fg underline-offset-2 hover:underline"
                    >
                      {one}
                    </Link>
                  ))}
                </span>
              ) : (
                <span className="text-fg-subtle">—</span>
              ),
          },
          {
            id: "phase",
            label: t("spaces.field.phase"),
            filter: true,
            text: (space) => space.status?.phase ?? "",
            cell: (space) => <LifecycleBadge kind="phase" value={space.status?.phase} />,
          },
          {
            // The biggest space is one sort away (T-2889): Entities, descending.
            id: "entities",
            label: t("spaces.field.entities"),
            numeric: true,
            align: "right",
            text: (space) => String(usageOf(space.metadata.name)?.data?.entities ?? ""),
            cell: (space) => (
              // Count and size end on one edge; the info button stands after the count (owner 2026-11-01).
              <span className="inline-grid grid-cols-[auto_auto] items-center gap-x-0.5 tabular-nums">
                <span className="justify-self-end">
                  <EntityCount usage={usageOf(space.metadata.name)} numbers={numbers} />
                </span>
                <UsageInfo project={project} space={space.metadata.name} usage={usageOf(space.metadata.name)?.data} locale={locale} />
                <span className="col-start-1 justify-self-end">
                <SizeLine
                  bytes={usageOf(space.metadata.name)?.data?.bytes?.entities ?? (usageOf(space.metadata.name)?.data?.entities === 0 ? 0 : undefined)}
                  known={usageOf(space.metadata.name)?.data !== undefined}
                  locale={locale}
                  label={t("spaces.field.size")}
                />
                </span>
              </span>
            ),
          },
          {
            id: "history",
            label: t("spaces.field.history"),
            numeric: true,
            align: "right",
            text: (space) => String(usageOf(space.metadata.name)?.data?.attrInstances ?? ""),
            cell: (space) => (
              <span className="inline-flex flex-col items-start tabular-nums sm:items-end">
                <HistoryCount usage={usageOf(space.metadata.name)?.data} numbers={numbers} locale={locale} />
              </span>
            ),
          },
        ]}
        actions={(space) => (
          <SpaceRowActions
            project={project}
            name={space.metadata.name}
            target={{
              project,
              kind: "ContextSpace",
              plural: "spaces",
              name: space.metadata.name,
              label: localized(space.metadata.title, locale, space.metadata.name),
            }}
            locale={locale}
            sourceUrl={space.status?.sourceUrl}
          />
        )}
        empty={
          <EmptyState bare icon="spaces" title={t("spaces.empty")} description={t("spaces.addHint")} action={<div className="flex flex-wrap gap-2">{addButtons}</div>} />
        }
      />

      {/* What the project holds against its quota comes after what it holds: for a first
          steward the list, empty or not, is the page, and the bar is secondary (T-1396). */}
      <ProjectQuota project={project} />

      <ResourceFormDialog<SpaceForm>
        kind="ContextSpace"
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        title={t("spaces.add")}
        description={t("spaces.addHint")}
        schema={contextSpaceSchema(t)}
        // The same YAML view, draft, Check and verdict every other form has (T-1380, PF-57):
        // strict validation refuses a space nobody checked, so the form checks it first.
        formData={form}
        onChange={setForm}
        project={project}
        draftKind="ContextSpace"
        draftName={urlDraftName}
        plural="spaces"
        source={{
          toManifest: (form) => toEnvelope(project, form),
          fromManifest: (manifest) => fromEnvelope(manifest, locale),
        }}
        submitLabel={t("spaces.propose")}
        // The form opens from an address or a draft as well as from the disabled button, so it
        // says the project is full itself, before anything is typed (T-2758, PF-75).
        submitDisabledReason={quotaExceeded ? t("quota.exceeded", { limit }) : undefined}
        submitting={create.isPending}
        error={formError}
        onSubmit={(form) => create.mutate(form)}
      >
        {quotaExceeded ? (
          <Alert role="status" tone="warning">
            {t("quota.exceeded", { limit })}
          </Alert>
        ) : null}
      </ResourceFormDialog>
    </div>
  );
}
