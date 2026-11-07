import { useCreateFormFromDraft } from "../components/forms/FormRoute";
import { RecordLink } from "../components/RecordLink";
import { Fragment, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap, whilePending } from "../api/client";
import { proposeChecked } from "../api/proposal";
import { asManifests, isChange, localized, storedMetadata } from "../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../api/manifest";
import { ChangeNotice } from "../components/ChangeNotice";
import { TypeLink } from "../pages/models/ModelLinks";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ResourceFormDialog } from "../components/ResourceFormDialog";
import { ManifestGrid } from "../components/grid/ManifestGrid";
import { ResourceRowActions } from "../components/ResourceRowActions";
import { subscriptionSchema, subscriptionUiSchema } from "../schemas/kinds";
import {
  Badge,
  Button,
  EmptyState,
  Icon,
  PageHeader,
} from "../components/ui";
import { PermissionGuard } from "../components/ui/PermissionGuard";
import { SPACE_LABEL } from "../schemas/modelprojection";

export interface SubscriptionForm {
  name: string;
  contextSpaceRef: string;
  subscriptionName?: string;
  description?: string;
  entities?: { type?: string; id?: string; idPattern?: string }[];
  watchedAttributes?: string[];
  q?: string;
  geoQ?: string;
  notification: {
    endpoint: {
      uri: string;
      accept?: string;
      receiverInfo?: { key: string; value: string }[];
      secretRef?: { name: string; key?: string };
    };
    format?: string;
    attributes?: string[];
  };
  throttling?: number;
  expiresAt?: string;
  isActive?: boolean;
}

/** Whether a value is worth writing: a manifest reads as what it watches, not as a list of blanks. */
function isPresent(value: unknown): boolean {
  if (value === undefined || value === null) return false;
  if (typeof value === "string") return value.trim() !== "";
  if (Array.isArray(value)) return value.length > 0;
  if (typeof value === "object") return Object.keys(present(value as Record<string, unknown>)).length > 0;
  return true;
}

/** An object without its empty members, recursively. */
function present(members: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(members)
      .map(([key, value]): [string, unknown] => [
        key,
        value && typeof value === "object" && !Array.isArray(value)
          ? present(value as Record<string, unknown>)
          : value,
      ])
      .filter(([, value]) => isPresent(value)),
  );
}

/** A selector with the members it names, and no blank one beside them. */
function selectors(entities: SubscriptionForm["entities"]): Record<string, unknown>[] {
  return (entities ?? [])
    .map((selector) => present(selector as Record<string, unknown>))
    .filter((selector) => Object.keys(selector).length > 0);
}

/** Whether the form watches anything: jc-core refuses a subscription that watches nothing (CC-72). */
export function watchesSomething(form: SubscriptionForm): boolean {
  return selectors(form.entities).length > 0 || (form.watchedAttributes ?? []).some((a) => a.trim() !== "");
}

/**
 * The form as the manifest the API stores. `stored` is the manifest an edit started from: its
 * title, description and labels travel on, because the form has no field for them and an edit
 * that dropped them would propose their deletion nobody asked for.
 */
export function toSubscriptionEnvelope(
  project: string,
  form: SubscriptionForm,
  stored?: unknown,
): unknown {
  const { name, contextSpaceRef, entities, isActive, ...rest } = form;
  const kept = storedMetadata(stored);
  const labels = {
    ...((kept.labels as Record<string, string> | undefined) ?? {}),
    ...(contextSpaceRef ? { [SPACE_LABEL]: contextSpaceRef } : {}),
  };
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Subscription",
    metadata: {
      ...kept,
      name,
      namespace: project,
      ...(Object.keys(labels).length > 0 ? { labels } : {}),
    },
    spec: {
      ...present(rest as Record<string, unknown>),
      contextSpaceRef: { kind: "ContextSpace", name: contextSpaceRef },
      ...(selectors(entities).length > 0 ? { entities: selectors(entities) } : {}),
      // `true` is the default and stays unwritten; a parked subscription says so (DS-16).
      ...(isActive === false ? { isActive: false } : {}),
    },
  };
}

/** The stored manifest back as the form: the same pair, so the YAML view and the fields agree. */
export function fromSubscriptionEnvelope(manifest: unknown): SubscriptionForm {
  const envelope = (manifest ?? {}) as {
    metadata?: { name?: string };
    spec?: Record<string, unknown>;
  };
  const spec = envelope.spec ?? {};
  const reference = spec.contextSpaceRef as string | { name?: string } | undefined;
  const notification = (spec.notification ?? {}) as Partial<SubscriptionForm["notification"]>;
  return {
    ...(spec as object),
    name: envelope.metadata?.name ?? "",
    contextSpaceRef: (typeof reference === "string" ? reference : reference?.name) ?? "",
    notification: {
      ...notification,
      endpoint: { uri: "", ...(notification.endpoint ?? {}) },
    },
    isActive: spec.isActive !== false,
  } as SubscriptionForm;
}

/** The secret names this project's subscriptions already carry, offered before a new one is typed. */
function secretNames(subscriptions: Manifest[]): string[] {
  const names = new Set<string>();
  for (const subscription of subscriptions) {
    const name = fromSubscriptionEnvelope(subscription).notification.endpoint.secretRef?.name;
    if (name) names.add(name);
  }
  return [...names].sort();
}

/**
 * What a row says a subscription watches: its types or ids, then its attributes. A type links to
 * the class of the model that carries it (T-2766).
 */
function Watched({ project, form }: { project: string; form: SubscriptionForm }): JSX.Element {
  const parts: ReactNode[] = [
    ...selectors(form.entities).map((selector, index) =>
      typeof selector.type === "string" ? (
        <TypeLink key={`t${index}`} project={project} type={selector.type} space={form.contextSpaceRef || undefined} />
      ) : (
        String(selector.id ?? selector.idPattern)
      ),
    ),
    ...(form.watchedAttributes ?? []),
  ];
  if (parts.length === 0) {
    return <>—</>;
  }
  return (
    <>
      {parts.map((part, index) => (
        <Fragment key={index}>
          {index === 0 ? null : ", "}
          {part}
        </Fragment>
      ))}
    </>
  );
}

/** What the space reports of a subscription's deliveries, in `status.notifications` (T-4866). */
export interface Deliveries {
  status?: string;
  timesSent?: number;
  timesFailed?: number;
  lastNotification?: string;
  lastSuccess?: string;
  lastFailure?: string;
}

/**
 * Whether the receiver gets what the subscription watches, in words: delivered with the last
 * time, failing with the last failure, or nothing sent yet. The tone never carries it alone.
 */
export function DeliveryCell({ deliveries, live }: { deliveries?: Deliveries; live: boolean }): JSX.Element {
  const { t, i18n } = useTranslation();
  if (!live) return <>—</>;
  const when = (at?: string) =>
    at
      ? new Intl.DateTimeFormat(i18n.resolvedLanguage ?? i18n.language ?? "en", {
          dateStyle: "medium",
          timeStyle: "medium",
        }).format(new Date(at))
      : "";
  const sent = deliveries?.timesSent ?? 0;
  const failed = deliveries?.timesFailed ?? 0;
  if (!deliveries?.lastNotification && sent === 0 && failed === 0) {
    return <span className="text-caption text-fg-muted">{t("subscriptions.delivery.none")}</span>;
  }
  const failing = deliveries?.status === "failed";
  return (
    <div className="flex flex-col items-start gap-1">
      <Badge tone={failing ? "danger" : "success"}>
        {failing ? t("subscriptions.delivery.failing") : t("subscriptions.delivery.ok")}
      </Badge>
      <span className="text-caption">
        {failing
          ? t("subscriptions.delivery.lastFailure", { at: when(deliveries?.lastFailure ?? deliveries?.lastNotification) })
          : t("subscriptions.delivery.lastSuccess", { at: when(deliveries?.lastSuccess ?? deliveries?.lastNotification) })}
      </span>
      <span className="text-caption text-fg-muted">
        {t("subscriptions.delivery.counts", { sent, failed })}
      </span>
      {failing ? (
        <span className="text-caption text-fg-muted">{t("subscriptions.delivery.failingHint")}</span>
      ) : null}
    </div>
  );
}

/**
 * The subscriptions of one project: what each one watches, in which space, and where its
 * notifications go (CC-72, DS-16, UI-01).
 *
 * A Subscription had no form, so a standing query was authored only as YAML (T-2344). The form
 * is the same `ResourceFormDialog` every kind uses, so the YAML view, the draft, the Check and the
 * verdict come with it, and a subscription is proposed as a Change rather than written (UI-44).
 */
export function SubscriptionsPage({ project, edit }: { project: string; edit?: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const queryClient = useQueryClient();
  // The create form is a page at `/{plural}/new` on a routed list (T-2474).
  const [dialogOpen, setDialogOpen, handedDraft] = useCreateFormFromDraft();
  const [form, setForm] = useState<SubscriptionForm | undefined>(undefined);
  const [formError, setFormError] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);

  const list = useQuery({
    queryKey: queryKeys.list(project, "subscriptions"),
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "subscriptions" } },
        }),
      ),
  });

  const spaces = useQuery({
    queryKey: queryKeys.list(project, "spaces"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "spaces" } },
        }),
      ),
  });
  const spaceNames = asManifests(spaces.data?.items ?? []).map((space) => space.metadata.name);
  const subscriptions = asManifests(list.data?.items ?? []);
  const schema = subscriptionSchema(t, spaceNames, secretNames(subscriptions));
  const titleOf = (row: Manifest) =>
    localized(row.metadata.title, locale, fromSubscriptionEnvelope(row).subscriptionName || row.metadata.name);

  const create = useMutation({
    mutationFn: async (form: SubscriptionForm) => {
      setFormError(null);
      return proposeChecked(
        project,
        "subscriptions",
        toSubscriptionEnvelope(project, form) as ResourceProposal,
        true,
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setDialogOpen(false);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "subscriptions") });
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

  const addButton = (
    <PermissionGuard project={project} kind="Subscription" verb="propose">
      <Button
        variant="primary"
        icon={<Icon name="plus" className="size-4" />}
        onClick={() => {
          setFormError(null);
          setForm(undefined);
          setDialogOpen(true);
        }}
      >
        {t("subscriptions.add")}
      </Button>
    </PermissionGuard>
  );

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        compact
        title={t("subscriptions.title")}
        description={t("subscriptions.lead")}
        actions={addButton}
      />

      {change ? <ChangeNotice change={change} project={project} /> : null}

      <ManifestGrid<Manifest>
        caption={t("subscriptions.title")}
        storageKey="subscriptions"
        searchLabel={t("grid.searchIn", { list: t("subscriptions.title") })}
        rows={subscriptions}
        rowKey={(row) => row.metadata.name}
        query={list}
        name={{
          id: "name",
          label: t("subscriptions.field.name"),
          text: (row) => titleOf(row),
          cell: (row) => (
            <div className="flex items-center gap-2">
              <RecordLink project={project} plural="subscriptions" name={row.metadata.name}>
                {titleOf(row)}
              </RecordLink>
            </div>
          ),
        }}
        columns={[
          {
            id: "space",
            label: t("subscriptions.field.space"),
            field: "contextSpaceRef",
            filter: true,
            text: (row) => fromSubscriptionEnvelope(row).contextSpaceRef,
            cell: (row) => <span className="font-mono text-caption">{fromSubscriptionEnvelope(row).contextSpaceRef || "—"}</span>,
          },
          {
            id: "watches",
            label: t("subscriptions.field.watches"),
            text: (row) => {
              const shape = fromSubscriptionEnvelope(row);
              return [...selectors(shape.entities).map((each) => String(each.type ?? each.id ?? each.idPattern ?? "")), ...(shape.watchedAttributes ?? [])].join(", ");
            },
            cell: (row) => (
              <span className="font-mono text-caption">
                <Watched project={project} form={fromSubscriptionEnvelope(row)} />
              </span>
            ),
          },
          {
            id: "receiver",
            label: t("subscriptions.field.receiver"),
            text: (row) => fromSubscriptionEnvelope(row).notification.endpoint.uri,
            cell: (row) => <span className="break-all font-mono text-caption">{fromSubscriptionEnvelope(row).notification.endpoint.uri || "—"}</span>,
          },
          {
            id: "deliveries",
            label: t("subscriptions.field.deliveries"),
            text: (row) => String((row.status as { notifications?: Deliveries } | undefined)?.notifications?.timesSent ?? ""),
            numeric: true,
            cell: (row) => (
              <DeliveryCell
                deliveries={(row.status as { notifications?: Deliveries } | undefined)?.notifications}
                live={row.status?.phase === "Live"}
              />
            ),
          },
          {
            id: "active",
            label: t("grid.field.active"),
            field: "isActive",
            filter: true,
            hidden: true,
            text: (row) => (fromSubscriptionEnvelope(row).isActive === false ? t("grid.off") : t("grid.on")),
          },
          { id: "throttling", label: t("grid.field.throttling"), field: "throttling", hidden: true, numeric: true, text: (row) => String((row.spec as { throttling?: number }).throttling ?? "") },
          { id: "expiresAt", label: t("grid.field.expiresAt"), field: "expiresAt", hidden: true, text: (row) => String((row.spec as { expiresAt?: string }).expiresAt ?? "") },
          {
            id: "phase",
            label: t("resourceList.phase"),
            filter: true,
            // One state per row (T-6115 review): a paused Subscription is applied (Live) but sends
            // nothing, so it reads as Paused rather than as Live beside a Paused badge.
            text: (row) => (fromSubscriptionEnvelope(row).isActive === false ? t("subscriptions.paused") : row.status?.phase ?? ""),
            cell: (row) =>
              fromSubscriptionEnvelope(row).isActive === false ? (
                <Badge tone="warning">{t("subscriptions.paused")}</Badge>
              ) : (
                <LifecycleBadge kind="phase" value={row.status?.phase} />
              ),
          },
        ]}
        actions={(subscription) => (
          <ResourceRowActions
            project={project}
            target={{ project, kind: "Subscription", plural: "subscriptions", name: subscription.metadata.name, label: titleOf(subscription) }}
            form={{
              schema,
              uiSchema: subscriptionUiSchema,
              fromManifest: (manifest) => fromSubscriptionEnvelope(manifest) as unknown as Record<string, unknown>,
              toManifest: (edited, stored) => toSubscriptionEnvelope(project, edited as unknown as SubscriptionForm, stored),
            }}
          />
        )}
        edit={{ project, plural: "subscriptions", kind: "Subscription", manifest: (row) => row, onChange: setChange }}
        empty={<EmptyState bare title={t("subscriptions.empty")} description={t("subscriptions.addHint")} action={addButton} />}
      />

      <ResourceFormDialog<SubscriptionForm>
        kind="Subscription"
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        title={t("subscriptions.add")}
        description={t("subscriptions.addHint")}
        schema={schema}
        uiSchema={subscriptionUiSchema}
        formData={form}
        onChange={setForm}
        project={project}
        draftKind="Subscription"
        draftName={edit ?? handedDraft}
        plural="subscriptions"
        source={{
          toManifest: (form) => toSubscriptionEnvelope(project, form),
          fromManifest: (manifest) => fromSubscriptionEnvelope(manifest),
        }}
        submitLabel={t("subscriptions.propose")}
        submitting={create.isPending}
        error={formError}
        onSubmit={(form) => {
          // The one rule a schema cannot say field by field: something has to be watched. Said
          // here, before a round trip, in the words the person can act on (CC-72).
          if (!watchesSomething(form)) {
            setFormError(t("subscriptions.watchesNothing"));
            return;
          }
          create.mutate(form);
        }}
      />
    </div>
  );
}
