import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, reasonOf } from "../../api/client";
import { isChange, ORG_NAMESPACE, storedMetadata } from "../../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import { proposeChecked } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Alert, Button, Field, Select } from "../../components/ui";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { useGroups } from "../access/Groups";
import { withGroupMember } from "../access/groupRoles";
import { PersonPicker } from "../../components/pickers/PersonPicker";
import { personLabel, usePersonNames } from "../../api/personNames";

import { defaultGroupOf, othersOf, withMember, withoutMember } from "./members";
import type { AppRole, RolesSpec, Subject } from "./members";

export { asUser, defaultGroupOf, membersOf, othersOf, withMember, withoutMember } from "./members";
export type { RolesSpec, Subject } from "./members";

function titleOf(role: AppRole, language: string): string {
  if (typeof role.title === "string") return role.title;
  return role.title?.[language] ?? role.title?.en ?? role.name;
}

interface GroupSpec {
  description?: string;
  members?: { user: string }[];
}

/** One proposal: the manifest and the project and plural it goes to. */
interface Proposal {
  project: string;
  plural: string;
  manifest: Manifest;
}

/**
 * Who holds which role of an application (AP-99, AP-119, ADR-N-031): each role with its
 * description, its default group `{app}-{role}` and that group's members, and the other groups
 * and people the App gives the role to. "Add person" and "Remove" propose the default group with
 * the new `members`; "Give this role to a group" and "Remove" propose the App with the new
 * `spec.access`. Both go through the one propose function, checked first (PF-57), and leave a
 * change to review; nothing here grants anything by itself.
 *
 * Whoever may propose the App may propose its default groups and no other group of the
 * organization (AP-119, enforced by the server), so every write control is guarded by `propose`
 * on `App` and disabled with the reason for anybody else (PF-50, UI-44).
 *
 * An App that declares no role shows nothing: there is nobody to add to anything.
 */
export function RolesAndMembers({ project, name }: { project: string; name: string }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const heading = useId();
  const queryClient = useQueryClient();
  const [change, setChange] = useState<{ change: Change; project: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const app = useQuery({
    queryKey: queryKeys.resource(project, "apps", name),
    queryFn: async () =>
      unwrap<unknown>(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "apps", name } },
        }),
      ),
    retry: false,
  });
  const propose = useMutation({
    mutationFn: ({ project: target, plural, manifest }: Proposal) =>
      proposeChecked(target, plural, manifest as ResourceProposal, false),
    onMutate: () => {
      setError(null);
      setChange(null);
    },
    onSuccess: (result, { project: target, plural, manifest }) => {
      if (isChange(result)) setChange({ change: result, project: target });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.resource(target, plural, manifest.metadata.name),
      });
    },
    onError: (err) => setError(reasonOf(err, t("app.error.generic"))),
  });

  const spec = ((app.data as { spec?: RolesSpec } | undefined)?.spec ?? {}) as RolesSpec;
  const roles = spec.roles ?? [];
  if (roles.length === 0) return null;

  const proposeApp = (next: RolesSpec) =>
    propose.mutate({
      project,
      plural: "apps",
      manifest: {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "App",
        metadata: { ...storedMetadata(app.data), name, namespace: project },
        spec: next,
      } as Manifest,
    });

  return (
    <section aria-labelledby={heading} className="space-y-3 rounded border border-border p-4">
      <h2 id={heading} className="text-lg font-semibold">
        {t("apps.roles.title")}
      </h2>
      <p className="text-sm text-fg-muted">{t("apps.roles.lead")}</p>
      {change && <ChangeNotice change={change.change} project={change.project} />}
      {error && (
        <Alert tone="danger" role="alert">
          {error}
        </Alert>
      )}
      <ul className="space-y-4">
        {roles.map((role) => {
          const group = defaultGroupOf(name, role.name);
          return (
            <RoleRow
              key={role.name}
              project={project}
              group={group}
              title={titleOf(role, i18n.language)}
              description={role.description}
              others={othersOf(spec, role.name, group)}
              busy={propose.isPending}
              onGroup={(stored, members) =>
                propose.mutate({
                  project: ORG_NAMESPACE,
                  plural: "groups",
                  manifest: {
                    apiVersion: "joinedcontext.com/v1alpha1",
                    kind: "Group",
                    metadata: { ...storedMetadata(stored), name: group, namespace: ORG_NAMESPACE },
                    spec: { ...((stored as { spec?: GroupSpec }).spec ?? {}), members },
                  } as Manifest,
                })
              }
              onGive={(other) => {
                const next = withMember(spec, role.name, { group: other });
                if (next) proposeApp(next);
              }}
              onTake={(subject) => proposeApp(withoutMember(spec, role.name, subject))}
            />
          );
        })}
      </ul>
    </section>
  );
}

function RoleRow({
  project,
  group,
  title,
  description,
  others,
  busy,
  onGroup,
  onGive,
  onTake,
}: {
  project: string;
  group: string;
  title: string;
  description?: string;
  others: Subject[];
  busy: boolean;
  /** Proposes the default group, as it is stored, with these members. */
  onGroup: (stored: unknown, members: { user: string }[]) => void;
  onGive: (group: string) => void;
  onTake: (subject: Subject) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const stored = useQuery({
    queryKey: queryKeys.resource(ORG_NAMESPACE, "groups", group),
    queryFn: async () =>
      unwrap<unknown>(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project: ORG_NAMESPACE, plural: "groups", name: group } },
        }),
      ),
    retry: false,
  });
  const groups = useGroups();
  const members = (stored.data as { spec?: GroupSpec } | undefined)?.spec?.members ?? [];
  // The realm's people the caller may list, each written as their Keycloak id (T-2685, ADR-N-061).
  const held = members.map((member) => member.user);
  const names = usePersonNames([...held, ...others.flatMap((subject) => (subject.user ? [subject.user] : []))]);
  const [person, setPerson] = useState("");
  const [picked, setPicked] = useState("");
  const addReason = !stored.data
    ? t("apps.roles.noDefaultGroup", { group })
    : person === ""
      ? t("apps.roles.needEmail")
      : held.includes(person)
        ? t("apps.roles.already")
        : undefined;
  const offered = ((groups.data?.items ?? []) as { metadata: { name: string } }[])
    .map((g) => g.metadata.name)
    .filter((name) => name !== group && !others.some((subject) => subject.group === name));
  const giveReason = groups.isError
    ? t("apps.roles.groupsUnreadable")
    : picked === ""
      ? t("apps.roles.needGroup")
      : undefined;

  return (
    <li className="space-y-3" aria-label={title}>
      <div>
        <h3 className="font-medium">
          {title}{" "}
          <span className="text-sm text-fg-muted">
            ({t("apps.roles.count", { count: members.length + others.length })})
          </span>
        </h3>
        {description && <p className="text-sm text-fg-muted">{description}</p>}
      </div>

      <div className="space-y-2">
        <h4 className="text-sm font-medium">{t("apps.roles.defaultGroup", { group })}</h4>
        {stored.isError ? (
          <p className="text-sm text-fg-muted">{t("apps.roles.noDefaultGroup", { group })}</p>
        ) : members.length === 0 ? (
          <p className="text-sm text-fg-muted">{t("apps.roles.nobody")}</p>
        ) : (
          <ul className="space-y-1">
            {members.map((member) => (
              <li key={member.user} className="flex items-center gap-2">
                <span>{personLabel(member.user, names)}</span>
                <PermissionGuard project={project} kind="App" verb="propose">
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={busy}
                    disabledReason={t("app.busyReason")}
                    aria-label={t("apps.roles.removeLabel", { member: personLabel(member.user, names), role: title })}
                    onClick={() =>
                      onGroup(
                        stored.data,
                        members.filter((kept) => kept.user !== member.user),
                      )
                    }
                  >
                    {t("apps.roles.remove")}
                  </Button>
                </PermissionGuard>
              </li>
            ))}
          </ul>
        )}
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            const next = person && stored.data ? withGroupMember(members, person) : null;
            if (next) {
              onGroup(stored.data, next);
              setPerson("");
            }
          }}
        >
          <Field id={`${ids}-person`} label={t("apps.roles.email")} description={t("apps.roles.personHint")}>
            <PersonPicker id={`${ids}-person`} label={t("apps.roles.email")} labelled value={person} exclude={held} onChange={setPerson} />
          </Field>
          <PermissionGuard project={project} kind="App" verb="propose">
            <Button type="submit" size="sm" disabled={busy || addReason !== undefined} disabledReason={addReason}>
              {t("apps.roles.addPerson")}
            </Button>
          </PermissionGuard>
        </form>
      </div>

      <div className="space-y-2">
        <h4 className="text-sm font-medium">{t("apps.roles.others")}</h4>
        {others.length === 0 ? (
          <p className="text-sm text-fg-muted">{t("apps.roles.noOthers")}</p>
        ) : (
          <ul className="space-y-1">
            {others.map((subject) => {
              const label = subject.user ? personLabel(subject.user, names) : t("apps.roles.groupMember", { group: subject.group });
              return (
                <li key={subject.user ?? `group:${subject.group}`} className="flex items-center gap-2">
                  <span>{label}</span>
                  <PermissionGuard project={project} kind="App" verb="propose">
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={busy}
                      disabledReason={t("app.busyReason")}
                      aria-label={t("apps.roles.removeLabel", { member: label, role: title })}
                      onClick={() => onTake(subject)}
                    >
                      {t("apps.roles.remove")}
                    </Button>
                  </PermissionGuard>
                </li>
              );
            })}
          </ul>
        )}
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            if (picked) {
              onGive(picked);
              setPicked("");
            }
          }}
        >
          <Field id={`${ids}-group`} label={t("apps.roles.group")}>
            <Select id={`${ids}-group`} value={picked} onChange={(event) => setPicked(event.target.value)}>
              <option value="">{t("apps.roles.pickGroup")}</option>
              {offered.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </Select>
          </Field>
          <PermissionGuard project={project} kind="App" verb="propose">
            <Button type="submit" size="sm" disabled={busy || giveReason !== undefined} disabledReason={giveReason}>
              {t("apps.roles.give")}
            </Button>
          </PermissionGuard>
        </form>
      </div>
    </li>
  );
}
