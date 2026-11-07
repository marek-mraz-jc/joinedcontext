import { useCreateFormFromDraft } from "../components/forms/FormRoute";
import { RecordLink } from "../components/RecordLink";
import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap, whilePending } from "../api/client";
import { proposeChecked } from "../api/proposal";
import { asManifests, isChange, localized, storedMetadata } from "../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../api/manifest";
import { ChangeNotice } from "../components/ChangeNotice";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { ResourceFormDialog } from "../components/ResourceFormDialog";
import { ResourceList } from "../components/ResourceList";
import { ResourceRowActions } from "../components/ResourceRowActions";
import { REGISTRATION_TARGETS, registrationSchema, registrationUiSchema } from "../schemas/kinds";
import type { RegistrationTarget } from "../schemas/kinds";
import {
  Button,
  EmptyState,
  Icon,
  PageHeader,
  RadioGroup,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Term,
} from "../components/ui";
import { PermissionGuard } from "../components/ui/PermissionGuard";
import { SPACE_LABEL } from "../schemas/modelprojection";

const KIND = "ContextSourceRegistration";
const PLURAL = "csrs";
export interface RegistrationForm {
  name: string;
  contextSpaceRef: string;
  endpointRef?: string;
  endpoint?: string;
  information: {
    entities: { type: string; id?: string; idPattern?: string }[];
    propertyNames?: string[];
    relationshipNames?: string[];
  }[];
  operations?: string[];
  mode?: string;
  federation?: { identity?: string; serviceAccountRef?: string };
  interval?: string;
  expiresAt?: string;
}

/** A reference as the form holds it: the name, whether the manifest wrote it bare or typed. */
function nameOf(reference: unknown): string | undefined {
  if (typeof reference === "string") return reference;
  const name = (reference as { name?: unknown } | null | undefined)?.name;
  return typeof name === "string" ? name : undefined;
}

const filled = (value: string | undefined): string | undefined =>
  value && value.trim() !== "" ? value.trim() : undefined;

/** Which of the two targets a registration names: the address when it has one, else the Endpoint. */
export function targetOf(form: Partial<RegistrationForm> | undefined): RegistrationTarget {
  return filled(form?.endpoint) ? "endpoint" : "endpointRef";
}

/**
 * The form as the manifest the API stores (MF-36, PF-48).
 *
 * Only the target the form holds is written, because jc-core refuses a registration naming both.
 * A `caller` identity carries no account: the caller's own token is forwarded, and an account
 * beside it would be one that is never used, which jc-core refuses as well.
 */
export function toRegistrationEnvelope(
  project: string,
  form: RegistrationForm,
  stored?: unknown,
): unknown {
  const endpoint = filled(form.endpoint);
  const endpointRef = endpoint ? undefined : filled(form.endpointRef);
  const identity = form.federation?.identity;
  const account = filled(form.federation?.serviceAccountRef);
  const interval = filled(form.interval);
  const expiresAt = filled(form.expiresAt);
  const information = (form.information ?? []).map((entry) => ({
    entities: (entry.entities ?? []).map((selector) => ({
      type: selector.type,
      ...(filled(selector.id) ? { id: filled(selector.id) } : {}),
      ...(filled(selector.idPattern) ? { idPattern: selector.idPattern } : {}),
    })),
    ...(entry.propertyNames && entry.propertyNames.length > 0
      ? { propertyNames: entry.propertyNames }
      : {}),
    ...(entry.relationshipNames && entry.relationshipNames.length > 0
      ? { relationshipNames: entry.relationshipNames }
      : {}),
  }));
  // What the form has no field for travels on from the manifest the edit started from (T-2470).
  const kept = storedMetadata(stored);
  const labels = {
    ...((kept.labels as Record<string, string> | undefined) ?? {}),
    ...(form.contextSpaceRef ? { [SPACE_LABEL]: form.contextSpaceRef } : {}),
  };
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: KIND,
    metadata: {
      ...kept,
      name: form.name,
      namespace: project,
      ...(Object.keys(labels).length > 0 ? { labels } : {}),
    },
    spec: {
      contextSpaceRef: { kind: "ContextSpace", name: form.contextSpaceRef },
      ...(endpointRef ? { endpointRef: { kind: "Endpoint", name: endpointRef } } : {}),
      ...(endpoint ? { endpoint } : {}),
      information,
      ...(form.operations && form.operations.length > 0 ? { operations: form.operations } : {}),
      ...(form.mode ? { mode: form.mode } : {}),
      ...(identity === "caller"
        ? { federation: { identity: "caller" } }
        : identity || account
          ? {
              federation: {
                identity: "serviceAccount",
                ...(account ? { serviceAccountRef: { kind: "ServiceAccount", name: account } } : {}),
              },
            }
          : {}),
      ...(interval ? { schedule: { interval } } : {}),
      ...(expiresAt ? { expiresAt } : {}),
    },
  };
}

/** The stored manifest back as the form: the same pair, so the YAML view and the fields agree. */
export function fromRegistrationEnvelope(manifest: unknown): RegistrationForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: Record<string, unknown> };
  const spec = envelope.spec ?? {};
  const federation = spec.federation as { identity?: string; serviceAccountRef?: unknown } | undefined;
  const schedule = spec.schedule as { interval?: string } | null | undefined;
  const account = nameOf(federation?.serviceAccountRef);
  return {
    name: envelope.metadata?.name ?? "",
    contextSpaceRef: nameOf(spec.contextSpaceRef) ?? "",
    ...(nameOf(spec.endpointRef) ? { endpointRef: nameOf(spec.endpointRef) } : {}),
    ...(typeof spec.endpoint === "string" ? { endpoint: spec.endpoint } : {}),
    information: (spec.information as RegistrationForm["information"] | undefined) ?? [],
    ...(Array.isArray(spec.operations) ? { operations: spec.operations as string[] } : {}),
    ...(typeof spec.mode === "string" ? { mode: spec.mode } : {}),
    ...(federation
      ? {
          federation: {
            identity: federation.identity ?? "serviceAccount",
            ...(account ? { serviceAccountRef: account } : {}),
          },
        }
      : {}),
    ...(schedule?.interval ? { interval: schedule.interval } : {}),
    ...(typeof spec.expiresAt === "string" ? { expiresAt: spec.expiresAt } : {}),
  };
}

/** The names of one project collection, for the lists the form offers. */
function useNames(project: string, plural: string): string[] {
  const list = useQuery({
    queryKey: queryKeys.list(project, plural),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
  });
  return asManifests(list.data?.items ?? []).map((item) => item.metadata.name);
}

const COLUMNS = 6;

/**
 * The context source registrations of one project: which space's broker answers with whose data
 * (MF-36, PF-48, SP-09, UI-01).
 *
 * A registration was the one federation manifest editable only as YAML, and the one whose mistakes
 * cost most: it decides what a hub space's audience reads from a member (T-2345). The form is the
 * same `ResourceFormDialog` every kind uses, so the YAML view, the draft, the Check and the verdict
 * come with it, and a registration is proposed as a Change rather than written (UI-44).
 */
export function RegistrationsPage({ project, edit }: { project: string; edit?: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const queryClient = useQueryClient();
  // The create form is a page at `/{plural}/new` on a routed list (T-2474).
  const [dialogOpen, setDialogOpen, handedDraft] = useCreateFormFromDraft();
  const [target, setTarget] = useState<RegistrationTarget>("endpointRef");
  const [form, setForm] = useState<RegistrationForm | undefined>(undefined);
  const [formError, setFormError] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);

  const list = useQuery({
    queryKey: queryKeys.list(project, PLURAL),
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: PLURAL } },
        }),
      ),
  });
  const spaces = useNames(project, "spaces");
  const endpoints = useNames(project, "endpoints");
  const accounts = useNames(project, "serviceaccounts");
  const uiSchema = registrationUiSchema(t);
  const schemaFor = (branch: RegistrationTarget, granted: string[] = []) =>
    registrationSchema(t, branch, spaces, endpoints, accounts, granted);

  const create = useMutation({
    mutationFn: async (form: RegistrationForm) => {
      setFormError(null);
      return proposeChecked(
        project,
        PLURAL,
        toRegistrationEnvelope(project, form) as ResourceProposal,
        true,
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setDialogOpen(false);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, PLURAL) });
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
    <PermissionGuard project={project} kind={KIND} verb="propose">
      <Button
        variant="primary"
        icon={<Icon name="plus" className="size-4" />}
        onClick={() => {
          setFormError(null);
          setForm(undefined);
          setTarget("endpointRef");
          setDialogOpen(true);
        }}
      >
        {t("registrations.add")}
      </Button>
    </PermissionGuard>
  );

  const registrations = asManifests(list.data?.items ?? []);

  return (
    <div className="flex flex-col gap-section">
      <PageHeader
        compact
        title={t("nav.csrs")}
        description={t("registrations.lead")}
        actions={addButton}
      />

      {change ? <ChangeNotice change={change} project={project} /> : null}

      <ResourceList
        query={list}
        caption={t("registrations.title")}
        head={
          <TableHead>
            <TableHeaderCell>{t("registrations.field.name")}</TableHeaderCell>
            <TableHeaderCell>
              <Term name="contextSpace">{t("registrations.field.space")}</Term>
            </TableHeaderCell>
            <TableHeaderCell>{t("registrations.column.source")}</TableHeaderCell>
            <TableHeaderCell>{t("registrations.column.covers")}</TableHeaderCell>
            <TableHeaderCell>{t("resourceList.phase")}</TableHeaderCell>
            <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
          </TableHead>
        }
        columns={COLUMNS}
        count={registrations.length}
        empty={
          <EmptyState
            bare
            title={t("registrations.empty")}
            description={t("registrations.addHint")}
            action={addButton}
          />
        }
      >
        {registrations.map((registration: Manifest) => {
          const shape = fromRegistrationEnvelope(registration);
          const title = localized(registration.metadata.title, locale, registration.metadata.name);
          const types = [
            ...new Set(shape.information.flatMap((entry) => (entry.entities ?? []).map((one) => one.type))),
          ];
          const branch = targetOf(shape);
          return (
            <TableRow key={registration.metadata.name}>
              <TableCell primary>
                <RecordLink project={project} plural="csrs" name={registration.metadata.name}>
                  {title}
                </RecordLink>
              </TableCell>
              <TableCell>
                <span className="font-mono text-caption">{shape.contextSpaceRef || "—"}</span>
              </TableCell>
              <TableCell>
                <div className="text-caption text-fg-muted">{t(`registrations.targetKind.${branch}`)}</div>
                <div className="break-all font-mono text-caption">
                  {(branch === "endpoint" ? shape.endpoint : shape.endpointRef) || "—"}
                </div>
              </TableCell>
              <TableCell>
                <span className="font-mono text-caption">{types.join(", ") || "—"}</span>
              </TableCell>
              <TableCell>
                <LifecycleBadge kind="phase" value={registration.status?.phase} />
              </TableCell>
              <TableCell align="right">
                <ResourceRowActions
                  project={project}
                  target={{ project, kind: KIND, plural: PLURAL, name: registration.metadata.name, label: title }}
                  form={{
                    schema: schemaFor(branch, shape.operations ?? []),
                    uiSchema,
                    fromManifest: (manifest) =>
                      fromRegistrationEnvelope(manifest) as unknown as Record<string, unknown>,
                    toManifest: (edited, stored) =>
                      toRegistrationEnvelope(project, edited as unknown as RegistrationForm, stored),
                  }}
                />
              </TableCell>
            </TableRow>
          );
        })}
      </ResourceList>

      <ResourceFormDialog<RegistrationForm>
        kind={KIND}
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        title={t("registrations.add")}
        description={t("registrations.addHint")}
        schema={schemaFor(target, form?.operations ?? [])}
        uiSchema={uiSchema}
        formData={form}
        onChange={(next) => {
          setForm(next);
          // A manifest pasted into the YAML view may name the other target; the form follows it.
          if (next && (filled(next.endpoint) || filled(next.endpointRef))) setTarget(targetOf(next));
        }}
        project={project}
        draftKind={KIND}
        draftName={edit ?? handedDraft}
        plural={PLURAL}
        source={{
          toManifest: (form) => toRegistrationEnvelope(project, form),
          fromManifest: (manifest) => fromRegistrationEnvelope(manifest),
        }}
        submitLabel={t("registrations.propose")}
        submitting={create.isPending}
        error={formError}
        onSubmit={(form) => create.mutate(form)}
      >
        <RadioGroup<RegistrationTarget>
          name="registration-target"
          legend={t("registrations.target")}
          value={target}
          options={REGISTRATION_TARGETS.map((value) => ({
            value,
            label: t(`registrations.targetKind.${value}`),
            description: t(`registrations.targetHint.${value}`),
          }))}
          onChange={(next) => {
            setTarget(next);
            // The other target goes, so the manifest never names both (MF-36).
            setForm((held) =>
              held
                ? { ...held, ...(next === "endpoint" ? { endpointRef: undefined } : { endpoint: undefined }) }
                : held,
            );
          }}
        />
      </ResourceFormDialog>
    </div>
  );
}
