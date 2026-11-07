import type { Manifest } from "../../api/manifest";
import { ORG_NAMESPACE } from "../../api/manifest";
import { bindingOf } from "./RoleBindings";
import type { RolesSpec, Subject } from "../apps/members";

/** Where a binding holds (PF-49): the organization, one project, or one space of a project. */
export type Place =
  | { kind: "organization" }
  | { kind: "project"; project: string }
  | { kind: "space"; project: string; space: string };

/** A role as the binding form offers it, and where the Role manifest lives (PF-68). */
export interface RoleChoice {
  name: string;
  /** `org` for an organization role, the project's name for a project role. */
  home: string;
}

/** One application role a group holds: which App of which project, which role (PF-95). */
export interface AppRoleHeld {
  project: string;
  app: string;
  role: string;
}

interface BindingSpec {
  subjects?: Subject[];
  role?: string;
  scope?: { organization?: string; project?: string; contextSpace?: string };
}

const namesGroup = (subject: Subject, group: string) => subject.group === group;

/** The bindings whose subjects name `group`, as the manifest lists them. */
export function bindingsNaming(bindings: Manifest[], group: string): Manifest[] {
  return bindings.filter((binding) =>
    ((binding.spec as BindingSpec).subjects ?? []).some((subject) => namesGroup(subject, group)),
  );
}

/**
 * The binding with `group` taken out of its subjects, or `null` when nobody would be left: a
 * binding naming nobody is removed rather than written empty (PF-95, jc-core refuses it).
 */
export function bindingWithoutGroup(binding: Manifest, group: string): Manifest | null {
  const spec = binding.spec as BindingSpec;
  const subjects = (spec.subjects ?? []).filter((subject) => !namesGroup(subject, group));
  if (subjects.length === 0) return null;
  return { ...binding, spec: { ...spec, subjects } } as Manifest;
}

/** Every role of every App naming `group` in its access, sorted by project, App, role. */
export function appRolesNaming(apps: { project: string; app: Manifest }[], group: string): AppRoleHeld[] {
  const held: AppRoleHeld[] = [];
  for (const { project, app } of apps) {
    for (const entry of (app.spec as RolesSpec).access ?? []) {
      if (entry.subjects.some((subject) => namesGroup(subject, group))) {
        held.push({ project, app: app.metadata.name, role: entry.role });
      }
    }
  }
  return held.sort((a, b) =>
    `${a.project}/${a.app}/${a.role}`.localeCompare(`${b.project}/${b.app}/${b.role}`),
  );
}

/**
 * Why `role` cannot be bound at `place`, or `null` when it can (PF-69): an organization role is
 * bound anywhere; a project's own role only in that project or one of its spaces.
 */
export function refusedPlace(role: RoleChoice, place: Place): "projectRoleAtOrganization" | "projectRoleElsewhere" | null {
  if (role.home === ORG_NAMESPACE) return null;
  if (place.kind === "organization") return "projectRoleAtOrganization";
  return place.project === role.home ? null : "projectRoleElsewhere";
}

/** The RoleBinding that gives `group` the role at `place`, as the Access page writes one. */
export function groupBinding(group: string, role: string, place: Place, organization: string | undefined): Manifest {
  const project = place.kind === "organization" ? ORG_NAMESPACE : place.project;
  return bindingOf(
    {
      subjectKind: "group",
      subject: group,
      role,
      place: place.kind === "space" ? `space:${place.space}` : place.kind,
      until: "",
    },
    project,
    organization,
  );
}

/** The members of a Group spec with the person `id` added, or `null` when they are one already. */
export function withGroupMember(members: { user: string }[], id: string): { user: string }[] | null {
  return withGroupMembers(members, [id]);
}

/**
 * The members with every new person of `ids` added once, or `null` when none is new (T-5605). A
 * member is their Keycloak user id, never an e-mail (ADR-N-061).
 */
export function withGroupMembers(members: { user: string }[], ids: string[]): { user: string }[] | null {
  const held = new Set(members.map((member) => member.user));
  const added = [...new Set(ids)].filter((id) => !held.has(id));
  return added.length === 0 ? null : [...members, ...added.map((user) => ({ user }))];
}
