import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap, reasonOf } from "../../api/client";
import { asManifests, isChange, ORG_NAMESPACE, storedMetadata } from "../../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import { useProjects } from "../../api/projects";
import { proposeChecked } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { Alert, Button, Card, Field, PageHeader, Select } from "../../components/ui";
import { ResourcePageFailed } from "../../components/ui/PageState";
import { DeniedNote, PermissionGuard } from "../../components/ui/PermissionGuard";
import { withMember, withoutMember } from "../apps/members";
import { PersonPicker } from "../../components/pickers/PersonPicker";
import { usePersonNames } from "../../api/personNames";
import type { RolesSpec } from "../apps/members";
import {
  appRolesNaming,
  bindingsNaming,
  bindingWithoutGroup,
  groupBinding,
  refusedPlace,
  withGroupMembers,
} from "./groupRoles";
import type { Place, RoleChoice } from "./groupRoles";
import { useOrganizationName } from "../../api/projects";
import { instancesReachedBy } from "../ckan/ckanAccess";
import { GroupDatasets } from "../organization/DataAccess";

interface GroupSpec {
  description?: string;
  members?: { user: string }[];
}

interface Scope {
  organization?: string;
  project?: string;
  contextSpace?: string;
}

/** One proposal: the manifest, where it goes, and whether it is new. */
interface Proposal {
  project: string;
  plural: string;
  manifest: Manifest;
  create: boolean;
}

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

/**
 * A group's page (PF-95, ADR-N-031): its members, the platform roles its bindings give it at
 * organization, project or space scope, and the application roles Apps give it. Every edit is a
 * manifest proposed through the one propose function, checked first (PF-57), and lands as a
 * Change in the lane the server gives it; nothing here grants anything by itself. A control the
 * caller's role does not allow stays in place, disabled, with the reason (UI-44).
 */
export function GroupPage({ name }: { name: string }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [change, setChange] = useState<{ change: Change; project: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const group = useQuery({
    queryKey: queryKeys.resource(ORG_NAMESPACE, "groups", name),
    queryFn: async () =>
      unwrap<unknown>(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project: ORG_NAMESPACE, plural: "groups", name } },
        }),
      ),
    retry: false,
  });
  const bindings = useList(ORG_NAMESPACE, "rolebindings");
  const projects = useProjects();
  const apps = useQueries({
    queries: (projects.data ?? []).map((project) => ({
      queryKey: queryKeys.list(project, "apps"),
      queryFn: async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: "apps" } },
          }),
        ),
    })),
  });

  // An open Change on this group: the next member edit is computed from the stored list, so a
  // second proposal beside it would drop what the first one adds (T-5605). The API reads no
  // manifest of an unmerged Change, so the page waits for its decision instead of guessing.
  const changes = useQuery({
    queryKey: queryKeys.changes(ORG_NAMESPACE),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/changes", { params: { path: { project: ORG_NAMESPACE } } })),
  });
  const waiting =
    (changes.data?.items ?? []).find((open) => {
      const params = open.summary.params as Record<string, unknown>;
      return open.status.phase === "PendingApproval" && params.kind === "Group" && params.name === name;
    })?.metadata.name ?? null;

  const propose = useMutation({
    mutationFn: ({ project, plural, manifest, create }: Proposal) =>
      proposeChecked(project, plural, manifest as ResourceProposal, create),
    onMutate: () => {
      setError(null);
      setChange(null);
    },
    onSuccess: (result, { project, plural, manifest }) => {
      if (isChange(result)) setChange({ change: result, project });
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, plural) });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.resource(project, plural, manifest.metadata.name),
      });
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
    onError: (err) => setError(reasonOf(err, t("app.error.generic"))),
  });

  if (group.isPending) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  if (group.error) {
    return (
      <ResourcePageFailed
        title={name}
        description={t("access.groupPage.lead")}
        error={group.error}
        onRetry={() => void group.refetch()}
        back={
          <Link to="/organization/$tab" params={{ tab: "groups" }} className="text-body underline">
            {t("access.groupPage.back")}
          </Link>
        }
      >
        {group.error instanceof ApiError && group.error.status === 404
          ? t("access.groupPage.notFound", { name })
          : undefined}
      </ResourcePageFailed>
    );
  }

  const stored = group.data as { spec?: GroupSpec } | undefined;
  const spec = stored?.spec ?? {};
  const members = spec.members ?? [];
  const naming = bindingsNaming(asManifests(bindings.data?.items ?? []), name);
  const everyApp = apps.flatMap((query, index) =>
    asManifests(query.data?.items ?? []).map((app) => ({
      project: (projects.data ?? [])[index] ?? "",
      app,
    })),
  );
  const appRoles = appRolesNaming(everyApp, name);

  const proposeGroup = (next: { user: string }[]) =>
    propose.mutate({
      project: ORG_NAMESPACE,
      plural: "groups",
      create: false,
      manifest: {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Group",
        metadata: { ...storedMetadata(group.data), name, namespace: ORG_NAMESPACE },
        spec: { ...spec, members: next },
      } as Manifest,
    });

  const proposeApp = (project: string, app: Manifest, next: RolesSpec) =>
    propose.mutate({
      project,
      plural: "apps",
      create: false,
      manifest: {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "App",
        metadata: { ...storedMetadata(app), name: app.metadata.name, namespace: project },
        spec: next,
      } as Manifest,
    });

  return (
    <div className="space-y-6">
      <PageHeader
        title={name}
        description={spec.description ?? t("access.groupPage.lead")}
        actions={
          <span className="inline-flex items-center gap-2">
            <Link to="/organization/$tab" params={{ tab: "groups" }} className="text-body underline">
              {t("access.groupPage.back")}
            </Link>
            <DeleteResourceAction
              label={t("access.groupPage.removeGroup")}
              target={{
                project: ORG_NAMESPACE,
                home: ORG_NAMESPACE,
                kind: "Group",
                plural: "groups",
                name,
                label: name,
              }}
            />
          </span>
        }
      />
      {naming.length + appRoles.length > 0 ? (
        <p className="text-body text-fg-muted">
          {t("access.groupPage.deleteCascade", { bindings: naming.length, apps: appRoles.length })}
        </p>
      ) : null}
      {change ? <ChangeNotice change={change.change} project={change.project} /> : null}
      {error ? (
        <Alert tone="danger" role="alert">
          {error}
        </Alert>
      ) : null}

      <Members
        members={members}
        busy={propose.isPending}
        waiting={waiting}
        onAdd={(ids) => {
          const next = withGroupMembers(members, ids);
          if (next) proposeGroup(next);
        }}
        onRemove={(id) => proposeGroup(members.filter((member) => member.user !== id))}
      />

      <GroupDatasets group={name} />

      <PlatformRoles
        group={name}
        bindings={naming}
        loading={bindings.isPending}
        busy={propose.isPending}
        onAdd={(manifest) => propose.mutate({ project: ORG_NAMESPACE, plural: "rolebindings", manifest, create: true })}
        onEdit={(manifest) => propose.mutate({ project: ORG_NAMESPACE, plural: "rolebindings", manifest, create: false })}
      />

      <CkanInstancesReached group={name} bindings={asManifests(bindings.data?.items ?? [])} />

      <ApplicationRoles
        group={name}
        held={appRoles}
        apps={everyApp}
        projects={projects.data ?? []}
        busy={propose.isPending}
        onAdd={(project, app, role) => {
          const next = withMember(app.spec as RolesSpec, role, { group: name });
          if (next) proposeApp(project, app, next);
        }}
        onRemove={(project, app, role) => proposeApp(project, app, withoutMember(app.spec as RolesSpec, role, { group: name }))}
      />
    </div>
  );
}

/**
 * A member as people read them (T-6423): the name, a link to their page. A caller who resolves no
 * name (ADR-N-061: names reach people-admins and group approvers only) reads a plain label, the
 * Keycloak id folded under it.
 */
export function MemberName({ id, names }: { id: string; names: Map<string, string> }): JSX.Element {
  const { t } = useTranslation();
  const name = names.get(id);
  if (!name) {
    return (
      <details className="min-w-0 text-body text-fg-muted">
        <summary className="focus-ring w-fit cursor-pointer rounded-sm">{t("access.groupPage.hiddenName")}</summary>
        <code className="block pt-1 font-mono text-caption text-fg [overflow-wrap:anywhere]">{id}</code>
      </details>
    );
  }
  return (
    <Link to="/organization/$tab/$" params={{ tab: "people", _splat: encodeURIComponent(id) }} className="text-body underline hover:no-underline">
      {name}
    </Link>
  );
}

function Members({
  members,
  busy,
  waiting,
  onAdd,
  onRemove,
}: {
  members: { user: string }[];
  busy: boolean;
  /** The id of a Change on this group that waits for a decision: no second edit is built beside it. */
  waiting: string | null;
  onAdd: (emails: string[]) => void;
  onRemove: (email: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const heading = useId();
  const field = useId();
  // The realm's people the caller may list, searched on the server, each written as their Keycloak
  // id; a pasted list of e-mails adds each person it finds (T-2685, T-4947, T-5605, ADR-N-061).
  const [chosen, setChosen] = useState<string[]>([]);
  const [refused, setRefused] = useState<string[]>([]);
  const held = members.map((member) => member.user);
  const names = usePersonNames(held);
  const adding = chosen.filter((id) => !held.includes(id));
  const blocked = waiting
    ? t("access.groupPage.waiting", { change: waiting })
    : adding.length === 0
      ? t("access.groupPage.needEmail")
      : undefined;
  return (
    <Card as="section" padding="md" aria-labelledby={heading} className="space-y-3">
      <h2 id={heading} className="text-title font-semibold text-fg">
        {t("access.groupPage.members")}
      </h2>
      {/* Why the greyed controls are off, in words beside them, on a phone too (T-6423). */}
      <DeniedNote project={ORG_NAMESPACE} kind="Group" verb="propose" />
      {waiting ? (
        <Alert tone="warning" title={t("access.groupPage.waitingTitle")}>
          {t("access.groupPage.waiting", { change: waiting })}{" "}
          <Link to="/projects/$project/approvals/$id" params={{ project: ORG_NAMESPACE, id: waiting }} className="underline">
            {t("access.groupPage.openChange", { change: waiting })}
          </Link>
        </Alert>
      ) : null}
      {members.length === 0 ? (
        <p className="text-body text-fg-muted">{t("access.groups.nobody")}</p>
      ) : (
        <ul className="space-y-1">
          {members.map((member) => (
            <li key={member.user} className="flex items-center gap-2">
              <MemberName id={member.user} names={names} />
              <PermissionGuard project={ORG_NAMESPACE} kind="Group" verb="propose">
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={busy || waiting !== null}
                  disabledReason={waiting ? t("access.groupPage.waiting", { change: waiting }) : t("app.busyReason")}
                  aria-label={t("access.groupPage.removeMember", { member: names.get(member.user) ?? member.user })}
                  onClick={() => onRemove(member.user)}
                >
                  {t("access.groupPage.remove")}
                </Button>
              </PermissionGuard>
            </li>
          ))}
        </ul>
      )}
      <form
        className="flex max-w-xl flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (blocked === undefined && !busy) {
            onAdd(adding);
            setChosen([]);
            setRefused([]);
          }
        }}
      >
        <Field
          id={field}
          label={t("access.groupPage.email")}
          description={t("access.groupPage.hint.email")}
          errors={refused.length > 0 ? [t("access.groupPage.notEmails", { words: refused.join(", ") })] : undefined}
        >
          <PersonPicker
            id={field}
            label={t("access.groupPage.email")}
            labelled
            multiple
            value={chosen}
            exclude={held}
            invalid={refused.length > 0}
            onChange={setChosen}
            onRefused={setRefused}
          />
        </Field>
        <PermissionGuard project={ORG_NAMESPACE} kind="Group" verb="propose">
          <Button type="submit" size="sm" className="self-start" disabled={busy || blocked !== undefined} disabledReason={blocked ?? t("app.busyReason")}>
            {adding.length > 1 ? t("access.groupPage.addMembers", { count: adding.length }) : t("access.groupPage.addMember")}
          </Button>
        </PermissionGuard>
      </form>
    </Card>
  );
}

function where(scope: Scope | undefined, t: (key: string, options?: Record<string, unknown>) => string): string {
  if (scope?.contextSpace) return t("access.roles.space", { name: scope.contextSpace });
  if (scope?.project) return t("access.roles.project", { name: scope.project });
  return t("access.groupPage.organization");
}

function PlatformRoles({
  group,
  bindings,
  loading,
  busy,
  onAdd,
  onEdit,
}: {
  group: string;
  bindings: Manifest[];
  loading: boolean;
  busy: boolean;
  onAdd: (manifest: Manifest) => void;
  onEdit: (manifest: Manifest) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const heading = useId();
  const ids = useId();
  // An organization place names the Organization the Access page is on (ADR-N-058, F-UI1-010).
  const organization = useOrganizationName(ORG_NAMESPACE);
  const projects = useProjects();
  const orgRoles = useList(ORG_NAMESPACE, "roles");
  const [target, setTarget] = useState(""); // "" = organization, else a project name
  const [space, setSpace] = useState("");
  const [role, setRole] = useState("");
  const spaces = useList(target, "spaces", target !== "");
  // Every project's own roles are offered, so a project role picked for the wrong place is
  // refused with the reason (PF-69) instead of vanishing from the list.
  const projectRoles = useQueries({
    queries: (projects.data ?? []).map((project) => ({
      queryKey: queryKeys.list(project, "roles"),
      queryFn: async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: "roles" } },
          }),
        ),
    })),
  });

  const choices: RoleChoice[] = [
    ...asManifests(orgRoles.data?.items ?? []).map((r) => ({ name: r.metadata.name, home: ORG_NAMESPACE })),
    ...projectRoles.flatMap((query, index) => {
      const home = (projects.data ?? [])[index] ?? "";
      return asManifests(query.data?.items ?? [])
        .filter((r) => (r.metadata.namespace ?? home) === home && home !== ORG_NAMESPACE)
        .map((r) => ({ name: r.metadata.name, home }));
    }),
  ];
  const place: Place =
    target === ""
      ? { kind: "organization" }
      : space
        ? { kind: "space", project: target, space }
        : { kind: "project", project: target };
  const chosen = choices.find((choice) => `${choice.home}/${choice.name}` === role);
  const refusal = chosen ? refusedPlace(chosen, place) : null;
  const reason = !chosen
    ? t("access.groupPage.needRole")
    : refusal
      ? t(`access.groupPage.refused.${refusal}`, { role: chosen.name })
      : undefined;

  return (
    <Card as="section" padding="md" aria-labelledby={heading} className="space-y-3">
      <h2 id={heading} className="text-title font-semibold text-fg">
        {t("access.groupPage.platformRoles")}
      </h2>
      <p className="text-body text-fg-muted">{t("access.groupPage.platformLead")}</p>
      <DeniedNote project={ORG_NAMESPACE} kind="RoleBinding" verb="propose" />
      {loading ? (
        <p role="status" className="text-body text-fg-muted">
          {t("app.loading")}
        </p>
      ) : bindings.length === 0 ? (
        <p className="text-body text-fg-muted">{t("access.groupPage.noPlatformRole")}</p>
      ) : (
        <ul className="space-y-1">
          {bindings.map((binding) => {
            const spec = binding.spec as { role?: string; scope?: Scope };
            const label = t("access.groupPage.roleAt", { role: spec.role ?? "", where: where(spec.scope, t) });
            const without = bindingWithoutGroup(binding, group);
            return (
              <li key={binding.metadata.name} className="flex flex-wrap items-center gap-2">
                <span>{label}</span>
                {without ? (
                  <PermissionGuard project={ORG_NAMESPACE} kind="RoleBinding" verb="propose">
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={busy}
                      disabledReason={t("app.busyReason")}
                      aria-label={t("access.groupPage.removeRole", { role: label })}
                      onClick={() => onEdit(without)}
                    >
                      {t("access.groupPage.remove")}
                    </Button>
                  </PermissionGuard>
                ) : (
                  <>
                    <span className="text-caption text-fg-muted">{t("access.groupPage.onlySubject")}</span>
                    <DeleteResourceAction
                      target={{
                        project: ORG_NAMESPACE,
                        home: ORG_NAMESPACE,
                        kind: "RoleBinding",
                        plural: "rolebindings",
                        name: binding.metadata.name,
                        label,
                      }}
                    />
                  </>
                )}
              </li>
            );
          })}
        </ul>
      )}
      <form
        className="flex max-w-xl flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (chosen && !refusal) {
            onAdd(groupBinding(group, chosen.name, place, organization));
            setRole("");
          }
        }}
      >
        <Field id={`${ids}-where`} label={t("access.groupPage.where")} description={t("access.groupPage.hint.where")}>
          <Select
            id={`${ids}-where`}
            value={target}
            onChange={(event) => {
              setTarget(event.target.value);
              setSpace("");
            }}
          >
            <option value="">{t("access.groupPage.organization")}</option>
            {(projects.data ?? []).map((project) => (
              <option key={project} value={project}>
                {t("access.roles.project", { name: project })}
              </option>
            ))}
          </Select>
        </Field>
        {target ? (
          <Field id={`${ids}-space`} label={t("access.groupPage.space")} description={t("access.groupPage.hint.space")}>
            <Select id={`${ids}-space`} value={space} onChange={(event) => setSpace(event.target.value)}>
              <option value="">{t("access.groupPage.wholeProject")}</option>
              {asManifests(spaces.data?.items ?? []).map((s) => (
                <option key={s.metadata.name} value={s.metadata.name}>
                  {s.metadata.name}
                </option>
              ))}
            </Select>
          </Field>
        ) : null}
        <Field id={`${ids}-role`} label={t("access.groupPage.role")} description={t("access.groupPage.hint.role")}>
          <Select id={`${ids}-role`} value={role} onChange={(event) => setRole(event.target.value)}>
            <option value="">{t("access.groupPage.pickRole")}</option>
            {choices.map((choice) => (
              <option key={`${choice.home}/${choice.name}`} value={`${choice.home}/${choice.name}`}>
                {choice.home === ORG_NAMESPACE
                  ? choice.name
                  : t("access.groupPage.projectRole", { role: choice.name, project: choice.home })}
              </option>
            ))}
          </Select>
        </Field>
        <PermissionGuard project={ORG_NAMESPACE} kind="RoleBinding" verb="propose">
          <Button type="submit" size="sm" className="self-start" disabled={busy || reason !== undefined} disabledReason={reason}>
            {t("access.groupPage.addRole")}
          </Button>
        </PermissionGuard>
      </form>
    </Card>
  );
}

function ApplicationRoles({
  group,
  held,
  apps,
  projects,
  busy,
  onAdd,
  onRemove,
}: {
  group: string;
  held: { project: string; app: string; role: string }[];
  apps: { project: string; app: Manifest }[];
  projects: string[];
  busy: boolean;
  onAdd: (project: string, app: Manifest, role: string) => void;
  onRemove: (project: string, app: Manifest, role: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const heading = useId();
  const ids = useId();
  const [project, setProject] = useState("");
  const [appName, setAppName] = useState("");
  const [role, setRole] = useState("");
  const find = (p: string, a: string) => apps.find((entry) => entry.project === p && entry.app.metadata.name === a)?.app;
  const inProject = apps.filter((entry) => entry.project === project);
  const app = find(project, appName);
  const roles = ((app?.spec as RolesSpec | undefined)?.roles ?? []).map((r) => r.name);
  const already = held.some((h) => h.project === project && h.app === appName && h.role === role);
  const reason = !app || !role ? t("access.groupPage.needAppRole") : already ? t("access.groupPage.already") : undefined;

  return (
    <Card as="section" padding="md" aria-labelledby={heading} className="space-y-3">
      <h2 id={heading} className="text-title font-semibold text-fg">
        {t("access.groupPage.appRoles")}
      </h2>
      <p className="text-body text-fg-muted">{t("access.groupPage.appLead", { group })}</p>
      <DeniedNote project={project || ORG_NAMESPACE} kind="App" verb="propose" />
      {held.length === 0 ? (
        <p className="text-body text-fg-muted">{t("access.groupPage.noAppRole")}</p>
      ) : (
        <ul className="space-y-1">
          {held.map((entry) => {
            const label = t("access.groupPage.appRole", entry);
            const manifest = find(entry.project, entry.app);
            return (
              <li key={`${entry.project}/${entry.app}/${entry.role}`} className="flex items-center gap-2">
                <Link
                  to="/projects/$project/$plural/$name"
                  params={{ project: entry.project, plural: "apps", name: entry.app }}
                  className="underline"
                >
                  {label}
                </Link>
                <PermissionGuard project={entry.project} kind="App" verb="propose">
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={busy || !manifest}
                    disabledReason={busy ? t("app.busyReason") : t("access.groupPage.appNotLoaded")}
                    aria-label={t("access.groupPage.removeAppRole", { role: label })}
                    onClick={() => manifest && onRemove(entry.project, manifest, entry.role)}
                  >
                    {t("access.groupPage.remove")}
                  </Button>
                </PermissionGuard>
              </li>
            );
          })}
        </ul>
      )}
      <form
        className="flex max-w-xl flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (app && role && !already) {
            onAdd(project, app, role);
            setRole("");
          }
        }}
      >
        <Field id={`${ids}-project`} label={t("access.groupPage.project")} description={t("access.groupPage.hint.project")}>
          <Select
            id={`${ids}-project`}
            value={project}
            onChange={(event) => {
              setProject(event.target.value);
              setAppName("");
              setRole("");
            }}
          >
            <option value="">{t("access.groupPage.pickProject")}</option>
            {projects.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </Select>
        </Field>
        <Field id={`${ids}-app`} label={t("access.groupPage.app")} description={t("access.groupPage.hint.app")}>
          <Select
            id={`${ids}-app`}
            value={appName}
            onChange={(event) => {
              setAppName(event.target.value);
              setRole("");
            }}
          >
            <option value="">{t("access.groupPage.pickApp")}</option>
            {inProject.map((entry) => (
              <option key={entry.app.metadata.name} value={entry.app.metadata.name}>
                {entry.app.metadata.name}
              </option>
            ))}
          </Select>
        </Field>
        <Field id={`${ids}-role`} label={t("access.groupPage.role")} description={t("access.groupPage.hint.appRole")}>
          <Select id={`${ids}-role`} value={role} onChange={(event) => setRole(event.target.value)}>
            <option value="">{t("access.groupPage.pickRole")}</option>
            {roles.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </Select>
        </Field>
        <PermissionGuard project={project || ORG_NAMESPACE} kind="App" verb="propose">
          <Button type="submit" size="sm" className="self-start" disabled={busy || reason !== undefined} disabledReason={reason}>
            {t("access.groupPage.addAppRole")}
          </Button>
        </PermissionGuard>
      </form>
    </Card>
  );
}

/**
 * The CKAN instances this group's bindings let it manage (PF-107), so an administrator answers
 * "what can this group touch" without reading YAML; each links to its project's catalogue page,
 * where its Access panel changes it.
 */
function CkanInstancesReached({ group, bindings }: { group: string; bindings: Manifest[] }): JSX.Element {
  const { t } = useTranslation();
  const heading = useId();
  const projects = useProjects();
  const roles = useList(ORG_NAMESPACE, "roles");
  const lists = useQueries({
    queries: (projects.data ?? []).map((project) => ({
      queryKey: queryKeys.list(project, "ckaninstances"),
      queryFn: async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/{plural}", {
            params: { path: { project, plural: "ckaninstances" } },
          }),
        ),
    })),
  });
  const instances = lists.flatMap((query, index) =>
    asManifests(query.data?.items ?? []).map((instance) => ({
      project: (projects.data ?? [])[index] ?? "",
      name: instance.metadata.name,
    })),
  );
  const reached = instancesReachedBy(group, asManifests(roles.data?.items ?? []), bindings, instances);
  const loading = projects.isPending || roles.isPending || lists.some((query) => query.isPending);
  const failed = roles.isError ? roles.error : lists.find((query) => query.isError)?.error;
  return (
    <Card as="section" padding="md" aria-labelledby={heading} className="space-y-3">
      <h2 id={heading} className="text-title font-semibold text-fg">
        {t("access.groupPage.ckanTitle")}
      </h2>
      <p className="text-body text-fg-muted">{t("access.groupPage.ckanLead", { group })}</p>
      {loading ? (
        <p role="status" className="text-body text-fg-muted">
          {t("app.loading")}
        </p>
      ) : failed ? (
        <Alert role="alert" tone="danger">
          {t("form.listFailed", { reason: reasonOf(failed, t("app.error.generic")) })}
        </Alert>
      ) : reached.length === 0 ? (
        <p className="text-body text-fg-muted">{t("access.groupPage.noCkan")}</p>
      ) : (
        <ul className="space-y-1">
          {reached.map((entry) => (
            <li key={`${entry.project}/${entry.name}/${entry.role}`} className="flex flex-wrap items-center gap-2">
              <Link to="/projects/$project/ckan" params={{ project: entry.project }} className="underline">
                {t("access.groupPage.ckanInstance", { project: entry.project, name: entry.name })}
              </Link>
              <span className="text-caption text-fg-muted">{t("access.groupPage.ckanVia", { role: entry.role })}</span>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}
