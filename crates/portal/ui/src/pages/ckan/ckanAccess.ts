import type { Manifest } from "../../api/manifest";
import { ORG_NAMESPACE } from "../../api/manifest";
import type { Subject } from "../apps/members";

/**
 * Who may manage one CKAN instance, and the manifests that change it (PF-107, Architecture/12
 * §2a). The right on instance `{name}` of project `{project}` is the Role
 * `ckan-admin-{project}-{name}`, one rule on `CkanInstance` constrained to that name, given by a
 * RoleBinding of the same name in that project. Nothing here is a new permission system: the
 * Portal, the forge and CI read these two kinds as they read every other role.
 */

export const CKAN_ADMIN_VERBS = ["propose", "approve", "delete"] as const;

/** Why a grant or a revocation is not written; each has its sentence under `ckan.access.refused`. */
export type AccessRefusal = "noInstance" | "unknownInstance" | "nameTooLong" | "noSubject" | "already" | "absent";

export class RefusedAccess extends Error {
  constructor(readonly reason: AccessRefusal) {
    super(reason);
    this.name = "RefusedAccess";
  }
}

const DNS_LABEL = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/;

/** `ckan-admin-{project}-{instance}`, refused when it is no Kubernetes name (63 characters). */
export function ckanAdminName(project: string, instance: string): string {
  if (instance.trim() === "") throw new RefusedAccess("noInstance");
  const name = `ckan-admin-${project}-${instance}`;
  if (name.length > 63 || !DNS_LABEL.test(name)) throw new RefusedAccess("nameTooLong");
  return name;
}

interface Constraint {
  field?: string;
  in?: string[];
  notIn?: string[];
  equals?: string;
  pattern?: string;
}

interface Rule {
  kinds?: string[];
  verbs?: string[];
  constraints?: Constraint[];
}

interface BindingSpec {
  subjects?: Subject[];
  role?: string;
  scope?: { organization?: string; project?: string; contextSpace?: string };
}

/** The Role that manages `instance` of `project` alone; refused for a name the project has no instance of. */
export function ckanAdminRole(project: string, instance: string, known: readonly string[]): Manifest {
  const name = ckanAdminName(project, instance);
  if (!known.includes(instance)) throw new RefusedAccess("unknownInstance");
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Role",
    metadata: { name, namespace: ORG_NAMESPACE },
    spec: {
      rules: [
        {
          kinds: ["CkanInstance"],
          verbs: [...CKAN_ADMIN_VERBS],
          constraints: [{ field: "metadata.name", in: [instance] }],
        },
      ],
    },
  } as Manifest;
}

/** A constraint read the way jc-core's `Constraint::holds` reads it, for a `metadata.name`. */
function holds(constraint: Constraint, value: string): boolean {
  if (constraint.field !== "metadata.name") return true; // a spec field narrows what is written, not which instance
  if (constraint.equals !== undefined) return value === constraint.equals;
  if (constraint.pattern !== undefined) {
    try {
      return new RegExp(`^(?:${constraint.pattern})$`).test(value);
    } catch {
      return false;
    }
  }
  if (constraint.in && constraint.in.length > 0) return constraint.in.includes(value);
  return !(constraint.notIn ?? []).includes(value);
}

/** Whether `role` lets its holder change `instance`: a write verb on `CkanInstance` whose constraints admit the name. */
export function grantsInstance(role: Manifest, instance: string): boolean {
  return ((role.spec as { rules?: Rule[] } | undefined)?.rules ?? []).some(
    (rule) =>
      (rule.kinds ?? []).includes("CkanInstance") &&
      (rule.verbs ?? []).some((verb) => (CKAN_ADMIN_VERBS as readonly string[]).includes(verb)) &&
      (rule.constraints ?? []).every((constraint) => holds(constraint, instance)),
  );
}

/** One subject who may change the instance, the role that says so and the binding it comes from. */
export interface Holder {
  subject: Subject;
  role: string;
  binding: string;
  /** Only the instance's own binding is edited from its Access panel; `org-admin` is not. */
  removable: boolean;
}

/**
 * Everyone who may change `instance` of `project`: the subjects of each binding at the
 * organization or in that project whose role grants it, groups first, then people, by name.
 */
export function holdersOf(project: string, instance: string, roles: Manifest[], bindings: Manifest[]): Holder[] {
  const own = ckanAdminName(project, instance);
  const granting = new Set(roles.filter((role) => grantsInstance(role, instance)).map((role) => role.metadata.name));
  const holders: Holder[] = [];
  for (const binding of bindings) {
    const spec = (binding.spec ?? {}) as BindingSpec;
    const covers = spec.scope?.organization !== undefined || spec.scope?.project === project;
    if (!covers || !spec.role || !granting.has(spec.role)) continue;
    for (const subject of spec.subjects ?? []) {
      holders.push({
        subject,
        role: spec.role,
        binding: binding.metadata.name,
        removable: binding.metadata.name === own && spec.role === own,
      });
    }
  }
  const key = (holder: Holder) => `${holder.subject.group ? 0 : 1}${holder.subject.group ?? holder.subject.user ?? ""}`;
  return holders.sort((a, b) => key(a).localeCompare(key(b)));
}

const sameSubject = (a: Subject, b: Subject) =>
  (a.group !== undefined && a.group === b.group) ||
  (a.user !== undefined && b.user !== undefined && a.user.toLowerCase() === b.user.toLowerCase());

/**
 * The manifests that give `subject` the right on `instance`: the Role when it is not there yet,
 * and the binding with the subject added (or a new one naming the subject alone).
 */
export function grantManifests(
  project: string,
  instance: string,
  subject: Subject,
  known: readonly string[],
  roles: Manifest[],
  bindings: Manifest[],
): { role: Manifest | null; binding: Manifest; create: boolean } {
  const name = ckanAdminName(project, instance);
  const role = ckanAdminRole(project, instance, known);
  if (!(subject.group ?? subject.user ?? "").trim()) throw new RefusedAccess("noSubject");
  const existing = bindings.find((binding) => binding.metadata.name === name);
  const missingRole = roles.some((r) => r.metadata.name === name) ? null : role;
  if (!existing) {
    return {
      role: missingRole,
      create: true,
      binding: {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "RoleBinding",
        metadata: { name, namespace: ORG_NAMESPACE },
        spec: { subjects: [subject], role: name, scope: { project } },
      } as Manifest,
    };
  }
  const spec = (existing.spec ?? {}) as BindingSpec;
  if ((spec.subjects ?? []).some((held) => sameSubject(held, subject))) throw new RefusedAccess("already");
  return {
    role: missingRole,
    create: false,
    binding: { ...existing, spec: { ...spec, subjects: [...(spec.subjects ?? []), subject] } } as Manifest,
  };
}

/**
 * The instance's binding without `subject`, or `null` when nobody would be left: a binding that
 * names nobody is removed, never written empty (jc-core refuses it).
 */
export function revokeManifest(binding: Manifest, subject: Subject): Manifest | null {
  const spec = (binding.spec ?? {}) as BindingSpec;
  const subjects = spec.subjects ?? [];
  if (!subjects.some((held) => sameSubject(held, subject))) throw new RefusedAccess("absent");
  const left = subjects.filter((held) => !sameSubject(held, subject));
  return left.length === 0 ? null : ({ ...binding, spec: { ...spec, subjects: left } } as Manifest);
}

/** The CKAN instances a group's bindings let it change, as `project/name`, for the group's page. */
export function instancesReachedBy(
  group: string,
  roles: Manifest[],
  bindings: Manifest[],
  instances: { project: string; name: string }[],
): { project: string; name: string; role: string }[] {
  const reached: { project: string; name: string; role: string }[] = [];
  for (const binding of bindings) {
    const spec = (binding.spec ?? {}) as BindingSpec;
    if (!(spec.subjects ?? []).some((subject) => subject.group === group)) continue;
    const role = roles.find((r) => r.metadata.name === spec.role);
    if (!role) continue;
    for (const instance of instances) {
      const covers = spec.scope?.organization !== undefined || spec.scope?.project === instance.project;
      if (covers && grantsInstance(role, instance.name)) {
        reached.push({ ...instance, role: role.metadata.name });
      }
    }
  }
  return reached.sort((a, b) => `${a.project}/${a.name}`.localeCompare(`${b.project}/${b.name}`));
}
