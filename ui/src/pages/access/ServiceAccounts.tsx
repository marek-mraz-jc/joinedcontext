import { PageFailed } from "../../components/ui/PageState";
import { useState } from "react";
import type { JSX } from "react";
import type { TFunction } from "i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, localized, ORG_NAMESPACE, storedMetadata } from "../../api/manifest";
import { useReadableList } from "../../api/permissions";
import { personLabel, usePersonNames } from "../../api/personNames";
import { useIdentity } from "../../auth/AuthProvider";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { FormRecordLink } from "../../components/RecordLink";
import { EditResourceAction } from "../../components/EditResourceDialog";
import { ChangeNotice } from "../../components/ChangeNotice";
import { useCreateForm } from "../../components/forms/FormRoute";
import { ResourceFormDialog } from "../../components/ResourceFormDialog";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { proposeChecked } from "../../api/proposal";
import {
  ROLE_SCOPE_LEVELS,
  serviceAccountSchema,
  serviceAccountUiSchema,
} from "../../schemas/kinds";
import { Button, Card, EmptyState, Icon, Skeleton } from "../../components/ui";
import { OPERATION_PRESETS } from "../../components/forms/widgets/accessPresets";
import { AccountAccess } from "./AccountAccess";
import { AccountSecret } from "./AccountSecret";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";

interface Credential {
  kind?: string;
  name?: string;
  expiresAt?: string;
}

/**
 * The grants of the brokers' roles an account can hold (ADR-N-048 §2.13): `mqtt/<broker>/<role>`
 * for each role a broker declares, and the same with `*` for the broker: every broker of the project
 * declaring it.
 */
export function mqttRoleChoices(brokers: Manifest[], t: TFunction): [string, string][] {
  const choices = new Map<string, string>();
  for (const broker of brokers) {
    for (const role of (broker.spec as { roles?: { name?: unknown }[] } | undefined)?.roles ?? []) {
      if (typeof role.name !== "string" || role.name === "") continue;
      choices.set(`mqtt/${broker.metadata.name}/${role.name}`, t("access.accounts.mqttRole", { role: role.name, broker: broker.metadata.name }));
      choices.set(`mqtt/*/${role.name}`, t("access.accounts.mqttRoleAny", { role: role.name }));
    }
  }
  return [...choices];
}

/** "space-reader in helsinki-kpi": a role and the one place it holds (PF-34). */
export function grantLabel(
  t: (key: string, options?: Record<string, unknown>) => string,
  role: string,
  scope: Record<string, string> | undefined,
): string {
  const where = scope?.contextSpace ?? scope?.project;
  if (where) {
    return t("access.accounts.grantIn", { role, where });
  }
  return scope?.organization !== undefined ? t("access.accounts.grantOrg", { role }) : role;
}

interface ServiceAccountSpec {
  owner?: { user?: string };
  purpose?: string;
  roles?: { role?: string; scope?: Record<string, string>; types?: string[] }[];
  credentials?: Credential[];
}

/** One grant as the form holds it: the scope is a level and a name, not three optional boxes. */
export interface ServiceAccountGrant {
  role: string;
  scope: { level: (typeof ROLE_SCOPE_LEVELS)[number]; name: string };
  operations?: string[];
  types?: string[];
  /** When the grant ends (AG-131); `notBefore` has no field and travels on unchanged. */
  notAfter?: string;
  notBefore?: string;
}

export interface ServiceAccountForm {
  name: string;
  purpose: string;
  owner: { user: string };
  roles: ServiceAccountGrant[];
  credentials: { kind: string; name: string; expiresAt?: string; ipAllowList?: string[] }[];
  limits?: { requestsPerMinute?: number };
  workload?: { kubernetes?: { namespace?: string; serviceAccount?: string } };
}

/** A list without blank entries, or nothing: the manifest reads as what it grants. */
function filled(values: string[] | undefined): string[] | undefined {
  const kept = (values ?? []).map((value) => value.trim()).filter((value) => value !== "");
  return kept.length > 0 ? kept : undefined;
}

/**
 * The form as the manifest the API stores. `stored` is the manifest an edit started from; its
 * title, description and labels travel on, because the form has no field for them.
 */
export function toServiceAccountEnvelope(
  project: string,
  form: ServiceAccountForm,
  stored?: unknown,
): unknown {
  const kubernetes = form.workload?.kubernetes;
  const perMinute = form.limits?.requestsPerMinute;
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ServiceAccount",
    metadata: { ...storedMetadata(stored), name: form.name, namespace: project },
    spec: {
      owner: { user: form.owner.user.trim() },
      purpose: form.purpose.trim(),
      roles: form.roles.map((grant) => {
        const operations = filled(grant.operations);
        const types = filled(grant.types);
        const notAfter = grant.notAfter?.trim();
        const notBefore = grant.notBefore?.trim();
        return {
          role: grant.role,
          scope: { [grant.scope.level]: grant.scope.name },
          ...(operations ? { operations } : {}),
          ...(types ? { types } : {}),
          ...(notAfter || notBefore
            ? { validity: { ...(notBefore ? { notBefore } : {}), ...(notAfter ? { notAfter } : {}) } }
            : {}),
        };
      }),
      credentials: form.credentials.map((credential) => {
        const ipAllowList = filled(credential.ipAllowList);
        return {
          kind: credential.kind,
          name: credential.name,
          ...(credential.expiresAt?.trim() ? { expiresAt: credential.expiresAt.trim() } : {}),
          ...(ipAllowList ? { ipAllowList } : {}),
        };
      }),
      ...(perMinute !== undefined ? { limits: { requestsPerMinute: perMinute } } : {}),
      ...(kubernetes?.namespace && kubernetes.serviceAccount
        ? {
            workload: {
              kubernetes: {
                namespace: kubernetes.namespace,
                serviceAccount: kubernetes.serviceAccount,
              },
            },
          }
        : {}),
    },
  };
}

function windowOf(validity: unknown): { notAfter?: string; notBefore?: string } {
  const window = (validity ?? {}) as { notAfter?: unknown; notBefore?: unknown };
  return {
    ...(typeof window.notAfter === "string" ? { notAfter: window.notAfter } : {}),
    ...(typeof window.notBefore === "string" ? { notBefore: window.notBefore } : {}),
  };
}

/** The stored manifest back as the form: the same pair, so the YAML view and the fields agree. */
export function fromServiceAccountEnvelope(manifest: unknown): ServiceAccountForm {
  const envelope = (manifest ?? {}) as { metadata?: { name?: string }; spec?: Record<string, unknown> };
  const spec = envelope.spec ?? {};
  const roles = (spec.roles as Record<string, unknown>[] | undefined) ?? [];
  return {
    name: envelope.metadata?.name ?? "",
    purpose: (spec.purpose as string | undefined) ?? "",
    owner: { user: ((spec.owner as { user?: string } | undefined)?.user) ?? "" },
    roles: roles.map((grant) => {
      const scope = (grant.scope as Record<string, string | null | undefined> | undefined) ?? {};
      // jc-core admits exactly one level, so the first one written is the one there is.
      const level = ROLE_SCOPE_LEVELS.find((candidate) => scope[candidate]) ?? "project";
      return {
        role: (grant.role as string | undefined) ?? "",
        scope: { level, name: scope[level] ?? "" },
        ...(grant.operations ? { operations: grant.operations as string[] } : {}),
        ...(grant.types ? { types: grant.types as string[] } : {}),
        // The window travels through an edit: dropping it would quietly widen the grant (AG-131).
        ...windowOf(grant.validity),
      };
    }),
    credentials: ((spec.credentials as ServiceAccountForm["credentials"] | undefined) ?? []).map(
      (credential) => ({ ...credential }),
    ),
    ...(spec.limits ? { limits: spec.limits as ServiceAccountForm["limits"] } : {}),
    ...(spec.workload ? { workload: spec.workload as ServiceAccountForm["workload"] } : {}),
  };
}

/** What the account may do, read when the person opens it: one request per opened account. */
function AccessDisclosure({ project, account }: { project: string; account: string }): JSX.Element {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <details className="mt-3" onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary className="cursor-pointer text-body font-medium text-fg">{t("access.agent.open")}</summary>
      {open ? (
        <div className="mt-2 space-y-4">
          <AccountAccess project={project} account={account} />
          <AccountSecret project={project} account={account} />
        </div>
      ) : null}
    </details>
  );
}

/** Every non-human caller of one project: who owns it, what it may do, and its credentials. */
export function ServiceAccounts({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const [dialogOpen, setDialogOpen] = useCreateForm();
  // A new account starts owned by the person creating it, which is who answers for it until
  // somebody else is named (PF-34); a grant on this project is the usual first one. The form
  // opened by its address (`/access/new`, T-2577) starts from the same. The owner is their
  // Keycloak id, the `sub` of their session, as the owner picker writes it (ADR-N-061).
  const newAccount = (): ServiceAccountForm => ({
    name: "",
    purpose: "",
    owner: { user: identity?.subject ?? "" },
    roles: [
      {
        role: "",
        scope: { level: "project", name: project },
        operations: [...OPERATION_PRESETS.reader],
      },
    ],
    credentials: [{ kind: "oauth-client", name: "" }],
  });
  const [form, setForm] = useState<ServiceAccountForm | undefined>(undefined);
  const [formError, setFormError] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const identity = useIdentity();
  const queryClient = useQueryClient();

  const create = useMutation({
    mutationFn: async (next: ServiceAccountForm) => {
      setFormError(null);
      return proposeChecked(
        project,
        "serviceaccounts",
        toServiceAccountEnvelope(project, next) as ResourceProposal,
        true,
      );
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      setDialogOpen(false);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "serviceaccounts") });
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
    <PermissionGuard project={project} kind="ServiceAccount" verb="propose">
      <Button
        variant="primary"
        icon={<Icon name="plus" className="size-4" />}
        onClick={() => {
          setFormError(null);
          setForm(newAccount());
          setDialogOpen(true);
        }}
      >
        {t("access.accounts.add")}
      </Button>
    </PermissionGuard>
  );

  const list = useQuery({
    queryKey: queryKeys.list(project, "serviceaccounts"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "serviceaccounts" } },
        }),
      ),
  });

  // The role is picked from the roles there are (T-2758): the organization's templates, which a
  // service account holds like a person does, and this project's own. A role an account already
  // holds stays a choice even when neither list names it, so an edit never loses it.
  const orgRoles = useReadableList(ORG_NAMESPACE, "roles", "Role");
  const projectRoles = useReadableList(project, "roles", "Role", project !== ORG_NAMESPACE);
  const roleChoices = new Map<string, string>();
  for (const role of asManifests([...(orgRoles.data?.items ?? []), ...(projectRoles.data?.items ?? [])])) {
    roleChoices.set(role.metadata.name, localized(role.metadata.title, locale, role.metadata.name));
  }
  // The roles the project's MQTT brokers declare (T-5876, ADR-N-048 §2.13), from the brokers the
  // person may read; the server checks each grant again when the account is proposed.
  const brokers = useReadableList(project, "mqttbrokers", "MqttBroker", project !== ORG_NAMESPACE);
  for (const [name, title] of mqttRoleChoices(asManifests(brokers.data?.items ?? []), t)) {
    roleChoices.set(name, title);
  }
  for (const account of asManifests(list.data?.items ?? [])) {
    for (const held of (account.spec as { roles?: { role?: unknown }[] }).roles ?? []) {
      if (typeof held.role === "string" && held.role !== "" && !roleChoices.has(held.role)) {
        roleChoices.set(held.role, held.role);
      }
    }
  }
  const ownerNames = usePersonNames(
    asManifests(list.data?.items ?? []).map((account) => (account.spec as ServiceAccountSpec).owner?.user ?? ""),
  );
  const schema = serviceAccountSchema(
    t,
    [project],
    [],
    // Until the organization's roles are read, the field stays a name to type: an empty list
    // would be a picker with nothing to pick.
    orgRoles.isSuccess ? [...roleChoices].map(([name, title]) => ({ name, title })) : [],
  );

  if (list.isPending) {
    return (
      <div role="status" aria-busy="true" aria-label={t("app.loading")} className="space-y-2">
        <Skeleton className="h-6 w-48" />
        <Skeleton className="h-24" />
      </div>
    );
  }
  if (list.isError) {
    return (
      <PageFailed error={list.error} onRetry={() => void list.refetch()} />
    );
  }

  const accounts: Manifest[] = asManifests(list.data.items ?? []);

  return (
    <section className="space-y-4" aria-labelledby="service-accounts-heading">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="service-accounts-heading" className="text-title font-semibold text-fg">
          {t("access.accounts.title")}
        </h2>
        {addButton}
      </div>

      {change ? <ChangeNotice change={change} project={project} /> : null}

{accounts.length === 0 ? (
        <EmptyState title={t(project === ORG_NAMESPACE ? "access.accounts.emptyOrganization" : "access.accounts.empty")} description={t(project === ORG_NAMESPACE ? "access.accounts.emptyHintOrganization" : "access.accounts.emptyHint")} icon="access" action={addButton} />
      ) : (
        <ul className="space-y-4">
          {accounts.map((account) => {
            const spec = account.spec as ServiceAccountSpec;
            const accountTarget = {
              project,
              kind: "ServiceAccount",
              plural: "serviceaccounts",
              name: account.metadata.name,
              label: localized(account.metadata.title, locale, account.metadata.name),
            };
            return (
              <li key={account.metadata.name}>
                <Card>
                  <div className="flex flex-wrap items-baseline justify-between gap-2">
                    <h3 className="font-medium text-fg">
                      <FormRecordLink name={account.metadata.name}>
                        {localized(account.metadata.title, locale, account.metadata.name)}
                      </FormRecordLink>
                    </h3>
                    {account.metadata.title ? (
                      <span className="font-mono text-caption text-fg-muted">
                        {account.metadata.name}
                      </span>
                    ) : null}
                    <span className="flex items-center gap-1.5">
                      <EditResourceAction
                        target={accountTarget}
                        addressed
                        form={{
                          schema,
                          uiSchema: serviceAccountUiSchema,
                          fromManifest: (manifest) =>
                            fromServiceAccountEnvelope(manifest) as unknown as Record<string, unknown>,
                          toManifest: (edited, stored) =>
                            toServiceAccountEnvelope(
                              project,
                              edited as unknown as ServiceAccountForm,
                              stored,
                            ),
                        }}
                      />
                      <DeleteResourceAction target={accountTarget} />
                    </span>
                  </div>
                  <dl className="mt-2 grid gap-x-6 gap-y-1 text-body sm:grid-cols-2">
                    <div className="flex gap-2">
                      <dt className="text-fg-muted">{t("access.accounts.owner")}</dt>
                      <dd className="font-medium text-fg">{personLabel(spec.owner?.user ?? "", ownerNames)}</dd>
                    </div>
                    <div className="flex gap-2">
                      <dt className="text-fg-muted">{t("access.accounts.roles")}</dt>
                      <dd className="font-medium text-fg">
                        {/* A role held in two places read "space-reader, space-reader" (T-2759):
                            each grant names where it holds. */}
                        {(spec.roles ?? [])
                          .filter((role) => Boolean(role.role))
                          .map((role) => grantLabel(t, role.role ?? "", role.scope))
                          .join(", ")}
                      </dd>
                    </div>
                    {spec.purpose ? (
                      <div className="flex gap-2 sm:col-span-2">
                        <dt className="text-fg-muted">{t("access.accounts.purpose")}</dt>
                        <dd className="text-fg">{spec.purpose}</dd>
                      </div>
                    ) : null}
                  </dl>
                  <AccessDisclosure project={project} account={account.metadata.name} />
                </Card>
              </li>
            );
          })}
        </ul>
      )}

      <ResourceFormDialog<ServiceAccountForm>
        kind="ServiceAccount"
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        title={t("access.accounts.add")}
        description={t("access.accounts.addHint")}
        schema={schema}
        uiSchema={serviceAccountUiSchema}
        formData={form ?? newAccount()}
        onChange={setForm}
        project={project}
        draftKind="ServiceAccount"
        plural="serviceaccounts"
        source={{
          toManifest: (next) => toServiceAccountEnvelope(project, next),
          fromManifest: (manifest) => fromServiceAccountEnvelope(manifest),
        }}
        submitLabel={t("access.accounts.propose")}
        submitting={create.isPending}
        error={formError}
        onSubmit={(next) => create.mutate(next)}
      />
    </section>
  );
}
