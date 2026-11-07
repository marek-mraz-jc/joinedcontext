import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, reasonOf } from "../../api/client";
import { asManifests, isChange, ORG_NAMESPACE } from "../../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import { usePermissions } from "../../api/permissions";
import { importChecked, proposeChecked } from "../../api/proposal";
import type { components } from "../../api/schema";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { Alert, Button, Field, Select } from "../../components/ui";
import { PersonPicker } from "../../components/pickers/PersonPicker";
import { isPersonId, personLabel, usePersonNames } from "../../api/personNames";
import type { Subject } from "../apps/members";
import { ckanAdminName, grantManifests, holdersOf, RefusedAccess, revokeManifest } from "./ckanAccess";

function useList(project: string, plural: string, enabled = true) {
  return useQuery({
    queryKey: queryKeys.list(project, plural),
    enabled,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
  });
}

/** The Role and its binding as one Change when both are new, else each on its own route. */
async function proposeGrant(project: string, grant: ReturnType<typeof grantManifests>): Promise<unknown> {
  if (grant.role && grant.create) {
    // The import lands each manifest in its own namespace: `org` for both (PF-68).
    const body: components["schemas"]["ImportOptions"] = {
      manifests: [grant.role, grant.binding] as ResourceProposal[],
    };
    return importChecked(project, body);
  }
  if (grant.role) {
    await proposeChecked(ORG_NAMESPACE, "roles", grant.role as ResourceProposal, true);
  }
  return proposeChecked(ORG_NAMESPACE, "rolebindings", grant.binding as ResourceProposal, grant.create);
}

/**
 * Who may manage one CKAN instance, and the one step that changes it (PF-107, T-3046): the groups
 * and people holding the right, each with the binding it comes from, the Endpoints that publish to
 * the instance, and an add/remove that proposes the instance's own Role and RoleBinding as a red
 * change (PF-52). Offered only to a person who may approve a binding; anyone else sees the list
 * and every control in place, disabled, with the reason (UI-44).
 */
export function CkanAccessPanel({
  project,
  instance,
  known,
}: {
  project: string;
  instance: string;
  /** The names of the project's instances as the caller read them: a grant names one of them. */
  known: readonly string[];
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const queryClient = useQueryClient();
  const permissions = usePermissions(ORG_NAMESPACE);
  // The holders are the organization's roles and bindings: a person who may not read them is
  // told who sees the list, rather than shown the 404 the organization answers them (T-3131).
  const mayRead = permissions.can("Role", "read") && permissions.can("RoleBinding", "read");
  const hidden = !permissions.isLoading && !mayRead;
  const roles = useList(ORG_NAMESPACE, "roles", !permissions.isLoading && mayRead);
  const bindings = useList(ORG_NAMESPACE, "rolebindings", !permissions.isLoading && mayRead);
  const groups = useList(ORG_NAMESPACE, "groups", !permissions.isLoading && permissions.can("Group", "read"));
  const endpoints = useList(project, "endpoints");
  const [kind, setKind] = useState<"group" | "user">("group");
  const [who, setWho] = useState("");
  const [refused, setRefused] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);

  const roleList = asManifests(roles.data?.items ?? []);
  const bindingList = asManifests(bindings.data?.items ?? []);
  let own: string | null = null;
  try {
    own = ckanAdminName(project, instance);
  } catch {
    own = null;
  }
  const holders = own ? holdersOf(project, instance, roleList, bindingList) : [];
  const ownBinding = bindingList.find((binding) => binding.metadata.name === own);
  const attached = asManifests(endpoints.data?.items ?? []).filter(
    (endpoint) =>
      (endpoint.spec as { publish?: { ckan?: { instanceRef?: { name?: string } } } } | undefined)?.publish?.ckan?.instanceRef
        ?.name === instance,
  );
  const mayManage = permissions.can("RoleBinding", "approve") && permissions.can("Role", "approve");
  const denied = permissions.isLoading
    ? t("app.loading")
    : mayManage
      ? undefined
      : t("ckan.access.denied");

  const after = (result: unknown) => {
    if (isChange(result)) setChange(result);
    void queryClient.invalidateQueries({ queryKey: queryKeys.list(ORG_NAMESPACE, "rolebindings") });
    void queryClient.invalidateQueries({ queryKey: queryKeys.list(ORG_NAMESPACE, "roles") });
  };
  const grant = useMutation({
    mutationFn: (manifests: ReturnType<typeof grantManifests>) => proposeGrant(project, manifests),
    onSuccess: (result) => {
      setWho("");
      after(result);
    },
  });
  const revoke = useMutation({
    mutationFn: (updated: Manifest) =>
      proposeChecked(ORG_NAMESPACE, "rolebindings", updated as ResourceProposal, false),
    onSuccess: after,
  });

  const submit = () => {
    setRefused(null);
    setChange(null);
    const typed = who.trim();
    // A person is their Keycloak user id, never their e-mail (ADR-N-061).
    const subject: Subject = kind === "group" ? { group: typed } : { user: typed };
    if (kind === "user" && typed !== "" && !isPersonId(typed)) {
      setRefused(t("ckan.access.refused.badEmail"));
      return;
    }
    try {
      grant.mutate(grantManifests(project, instance, subject, known, roleList, bindingList));
    } catch (error) {
      if (error instanceof RefusedAccess) {
        setRefused(t(`ckan.access.refused.${error.reason}`, { instance }));
        return;
      }
      throw error;
    }
  };

  const remove = (subject: Subject) => {
    setRefused(null);
    setChange(null);
    if (!ownBinding) return;
    try {
      const updated = revokeManifest(ownBinding, subject);
      if (updated) revoke.mutate(updated);
    } catch (error) {
      if (error instanceof RefusedAccess) {
        setRefused(t(`ckan.access.refused.${error.reason}`, { instance }));
        return;
      }
      throw error;
    }
  };

  const failure = grant.error ?? revoke.error;
  const loading = permissions.isLoading || (mayRead && (roles.isPending || bindings.isPending));
  const listFailed = roles.isError || bindings.isError;
  const busy = grant.isPending || revoke.isPending;
  const heading = `${ids}-heading`;
  const names = usePersonNames(holders.flatMap((holder) => (holder.subject.user ? [holder.subject.user] : [])));
  const name = (subject: Subject) => subject.group ?? personLabel(subject.user ?? "", names);

  return (
    <section aria-labelledby={heading} className="space-y-3 rounded border border-border p-4">
      <h3 id={heading} className="text-body font-semibold text-fg">
        {t("ckan.access.title", { instance })}
      </h3>
      <p className="text-body text-fg-muted">{t("ckan.access.lead")}</p>
      {own === null ? <Alert tone="warning">{t("ckan.access.refused.nameTooLong")}</Alert> : null}
      {change ? <ChangeNotice change={change} project={ORG_NAMESPACE} /> : null}
      {refused ? (
        <Alert role="alert" tone="danger">
          {refused}
        </Alert>
      ) : null}
      {failure ? (
        <Alert role="alert" tone="danger">
          {reasonOf(failure, t("app.error.generic"))}
        </Alert>
      ) : null}

      {loading ? (
        <p role="status" className="text-body text-fg-muted">
          {t("app.loading")}
        </p>
      ) : hidden ? (
        <p className="text-body text-fg-muted" data-testid="ckan-access-hidden">
          {t("ckan.access.hidden")}
        </p>
      ) : listFailed ? (
        <Alert role="alert" tone="danger">
          {t("form.listFailed", { reason: reasonOf(roles.error ?? bindings.error, t("app.error.generic")) })}
        </Alert>
      ) : holders.length === 0 ? (
        <p className="text-body text-fg-muted">{t("ckan.access.nobody")}</p>
      ) : (
        <ul className="space-y-1" aria-label={t("ckan.access.holders", { instance })}>
          {holders.map((holder) => {
            const label = holder.subject.group
              ? t("ckan.access.group", { name: holder.subject.group })
              : t("ckan.access.person", { name: personLabel(holder.subject.user ?? "", names) });
            const right =
              holder.role === own ? t("ckan.access.rightOwn") : t("ckan.access.rightVia", { role: holder.role });
            const last = holder.removable && (ownBinding?.spec as { subjects?: unknown[] } | undefined)?.subjects?.length === 1;
            return (
              <li key={`${holder.binding}/${name(holder.subject)}`} className="flex flex-wrap items-center gap-2">
                {holder.subject.group ? (
                  <Link
                    to="/organization/$tab/$"
                    params={{ tab: "groups", _splat: encodeURIComponent(holder.subject.group) }}
                    className="underline"
                  >
                    {label}
                  </Link>
                ) : (
                  <span>{label}</span>
                )}
                <span className="text-caption text-fg-muted">{right}</span>
                <span className="font-mono text-caption text-fg-subtle">
                  {t("ckan.access.binding", { name: holder.binding })}
                </span>
                {!holder.removable ? null : last ? (
                  <DeleteResourceAction
                    target={{
                      project: ORG_NAMESPACE,
                      home: ORG_NAMESPACE,
                      kind: "RoleBinding",
                      plural: "rolebindings",
                      name: holder.binding,
                      label: t("ckan.access.removeLast", { name: name(holder.subject), instance }),
                    }}
                  />
                ) : (
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={busy || denied !== undefined}
                    disabledReason={denied}
                    aria-label={t("ckan.access.remove", { name: name(holder.subject), instance })}
                    onClick={() => remove(holder.subject)}
                  >
                    {t("ckan.access.removeShort")}
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      )}

      <form
        className="flex flex-wrap items-end gap-2"
        // The form's own refusal, in words, rather than the browser's bubble for `type="email"`.
        noValidate
        aria-label={t("ckan.access.add", { instance })}
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <Field id={`${ids}-kind`} label={t("ckan.access.kind")} help={t("ckan.access.kindHelp")}>
          <Select
            id={`${ids}-kind`}
            value={kind}
            onChange={(event) => {
              setKind(event.target.value as "group" | "user");
              setWho("");
              setRefused(null);
            }}
          >
            <option value="group">{t("ckan.access.kindGroup")}</option>
            <option value="user">{t("ckan.access.kindPerson")}</option>
          </Select>
        </Field>
        {kind === "group" ? (
          <Field id={`${ids}-who`} label={t("ckan.access.whichGroup")} help={t("ckan.access.whichGroupHelp")}>
            <Select id={`${ids}-who`} value={who} onChange={(event) => setWho(event.target.value)}>
              <option value="">{t("ckan.access.pickGroup")}</option>
              {asManifests(groups.data?.items ?? []).map((group) => (
                <option key={group.metadata.name} value={group.metadata.name}>
                  {group.metadata.name}
                </option>
              ))}
            </Select>
          </Field>
        ) : (
          <Field id={`${ids}-who`} label={t("ckan.access.email")} help={t("ckan.access.emailHelp")}>
            <PersonPicker id={`${ids}-who`} label={t("ckan.access.email")} labelled value={who} onChange={setWho} />
          </Field>
        )}
        <Button
          type="submit"
          size="sm"
          loading={grant.isPending}
          disabled={busy || denied !== undefined || own === null}
          disabledReason={denied}
        >
          {t("ckan.access.addShort")}
        </Button>
      </form>

      <div className="space-y-1">
        <h4 className="text-caption font-semibold text-fg">{t("ckan.access.attached")}</h4>
        {endpoints.isPending ? (
          <p role="status" className="text-caption text-fg-muted">
            {t("app.loading")}
          </p>
        ) : endpoints.isError ? (
          <Alert role="alert" tone="danger">
            {t("form.listFailed", { reason: reasonOf(endpoints.error, t("app.error.generic")) })}
          </Alert>
        ) : attached.length === 0 ? (
          <p className="text-caption text-fg-muted">{t("ckan.access.noneAttached")}</p>
        ) : (
          <ul className="flex flex-wrap gap-2">
            {attached.map((endpoint) => (
              <li key={endpoint.metadata.name}>
                <Link
                  to="/projects/$project/$plural/$name"
                  params={{ project, plural: "endpoints", name: endpoint.metadata.name }}
                  className="underline"
                >
                  {endpoint.metadata.name}
                </Link>
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}
