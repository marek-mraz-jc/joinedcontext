import { useQuery } from "@tanstack/react-query";
import { api, ApiError, queryKeys, unwrap } from "./client";
import { ORG_NAMESPACE } from "./manifest";
import type { components } from "./schema";

/** What the caller may do in one project, from the bindings of the organization repository (PF-50). */
export type Effective = components["schemas"]["Effective"];
/** `create`, `update` and `disable` are valid on `Person` only (PF-91). */
export type Verb = "read" | "propose" | "approve" | "delete" | "create" | "update" | "disable";

export interface Rule {
  kinds?: string[];
  verbs?: string[];
}

/**
 * Whether the person administers the organization (PF-03): `approve` and `delete` on
 * `RoleBinding` in the organization's permissions, the question the server asks before the
 * validation health (OPS-53) and the organization-level Endpoints (PF-61). Unlike `allows`, no
 * document yet means no: what only an administrator sees is not shown while it is unknown.
 */
export function administersOrganization(effective: Effective | undefined): boolean {
  if (effective === undefined) {
    return false;
  }
  // A grant narrowed to one Organization's own settings makes nobody the installation's
  // administrator (ADR-N-058 section 2.4), as `Effective::administers_organization` decides.
  const installationWide = Array.isArray(effective.grants)
    ? { ...effective, grants: effective.grants.filter((grant) => !grant.organization) }
    : effective;
  return allows(installationWide, "RoleBinding", "approve") && allows(installationWide, "RoleBinding", "delete");
}

/**
 * The Organization whose own settings, bindings and groups the caller administers without
 * administering the installation (ADR-N-058 section 2.4, T-5248): the `organization` of their
 * narrowed grants at the organization namespace, else `undefined`.
 */
export function ownOrganizationOf(effective: Effective | undefined): string | undefined {
  if (effective === undefined || !Array.isArray(effective.grants) || administersOrganization(effective)) {
    return undefined;
  }
  return effective.grants.find((grant) => grant.organization)?.organization ?? undefined;
}

/** A grant allows a verb on a kind when its rule names both; `*` matches any kind. */
export function allows(effective: Effective | undefined, kind: string, verb: Verb): boolean {
  // Not (yet) a permissions document: the control shows and the API decides (PF-51 says the
  // UI is never the point of enforcement). Only a document that lists no grant hides it.
  if (!effective || !Array.isArray(effective.grants) || effective.bootstrap === true) {
    return true;
  }
  return effective.grants.some((grant) => {
    const rule = grant.rule as Rule;
    return (kind === "*" || rule.kinds?.includes(kind)) && Boolean(rule.verbs?.includes(verb));
  });
}

/**
 * Whether a grant lets the caller `verb` on `kind` in `space`: one of the project, or one bound
 * to that very space (the server's `Effective::may_in`, T-4593). `propose` implies `read`. No
 * document yet means yes, as `allows` reads it; the API still decides.
 */
export function allowsIn(effective: Effective | undefined, kind: string, verb: Verb, space: string): boolean {
  if (!effective || !Array.isArray(effective.grants) || effective.bootstrap === true) {
    return true;
  }
  return effective.grants.some((grant) => {
    const rule = grant.rule as Rule;
    const verbs = rule.verbs ?? [];
    return (
      Boolean(rule.kinds?.includes(kind)) &&
      (verbs.includes(verb) || (verb === "read" && verbs.includes("propose"))) &&
      (!grant.space || grant.space === space)
    );
  });
}

/**
 * Whether any grant lets the caller propose something in the project (T-4263): a create address
 * nothing opened tells a role without one that it may look, not create, instead of a warning that
 * the address might be wrong. No document yet means yes, as `allows` reads it.
 */
export function proposesAnything(effective: Effective | undefined): boolean {
  if (!effective || !Array.isArray(effective.grants) || effective.bootstrap === true) {
    return true;
  }
  return effective.grants.some((grant) => Boolean((grant.rule as Rule).verbs?.includes("propose")));
}

/**
 * Whether a grant lets the caller read `kind` (PF-59): `read`, or `propose`, which implies it
 * (jc-core `Rule::grants`). `undefined` while no document has arrived: a page that must not
 * fetch a list for a caller who may not read it waits instead of guessing (T-2605).
 */
export function mayRead(effective: Effective | undefined, kind: string): boolean | undefined {
  if (!effective || !Array.isArray(effective.grants)) {
    return undefined;
  }
  if (effective.bootstrap === true) {
    return true;
  }
  return effective.grants.some((grant) => {
    const rule = grant.rule as Rule;
    return Boolean(rule.kinds?.includes(kind)) && Boolean(rule.verbs?.includes("read") || rule.verbs?.includes("propose"));
  });
}

/** How many empty answers are asked again: a minute at five seconds, the first mirror sync's window. */
export const EMPTY_GRANTS_ASKS = 12;

/**
 * When to ask `permissions/me` again: every five seconds while the answer holds no grant, as a
 * fresh Portal pod answers before its first mirror sync (T-0630), and no more after
 * EMPTY_GRANTS_ASKS answers, since a person who genuinely holds nothing would be polled for as
 * long as the page is open (F-UI1-021).
 */
export function emptyGrantsRetry(
  data: { grants?: unknown; bootstrap?: boolean } | undefined,
  answers: number,
): number | false {
  const empty = data !== undefined && Array.isArray(data.grants) && data.grants.length === 0 && data.bootstrap !== true;
  return empty && answers < EMPTY_GRANTS_ASKS ? 5000 : false;
}

/**
 * The caller's effective permissions in `project` (T-0526). A convenience for the UI: a
 * control renders only when the API would honour it, and the API decides on its own.
 */
export function usePermissions(project: string) {
  const query = useQuery({
    queryKey: queryKeys.permissions(project),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/permissions/me", {
          params: { path: { project } },
        }),
      ),
    enabled: project !== "",
    // A fresh Portal pod answers with no grant until its first mirror sync; a page opened in
    // that window would otherwise keep every control disabled until a reload (T-0630).
    refetchInterval: (query) => emptyGrantsRetry(query.state.data, query.state.dataUpdateCount),
  });
  return {
    data: query.data,
    isLoading: query.isPending,
    // The first read is on its way: no document yet and none failed (T-5833). A disabled query
    // (no project) is never checking.
    checking: query.isLoading,
    // The 404 of a project no binding of the caller covers (PF-59): a known answer, reading nothing.
    denied: query.error instanceof ApiError && query.error.status === 404,
    can: (kind: string, verb: Verb) => allows(query.data, kind, verb),
  };
}

/**
 * Whether the caller administers the organization (PF-03, UI-75): `approve` and `delete` on
 * `RoleBinding` in the organization's permissions, as the seeded `org-admin` holds them and the
 * server asks. `known` is false until the document is read, and a document that never came means
 * no: what only an administrator sees is not shown while it is unknown. `opensAdministration`
 * also holds for an Organization's own administrator, who opens its own sections (T-5248).
 */
export function useAdministers(): { known: boolean; administers: boolean; own?: string; opensAdministration: boolean } {
  const permissions = usePermissions(ORG_NAMESPACE);
  const administers = administersOrganization(permissions.data);
  const own = ownOrganizationOf(permissions.data);
  return { known: !permissions.isLoading, administers, own, opensAdministration: administers || own !== undefined };
}

/** One rule of a `Role` as the form holds it: the verbs it grants on the kinds it names. */
export interface GrantedRule {
  kinds?: string[];
  verbs?: string[];
  constraints?: unknown[];
}

/**
 * What a proposed `Role` would grant beyond what the caller holds, as `verb on Kind` (PF-52).
 *
 * The same rule as `permissions::within_own_rights` on the server, read here so the form says it
 * before the proposal is sent: every verb on every kind a rule grants has to be one the caller
 * already holds, under no constraint their own grant adds. It never decides — an empty list means
 * "nothing this side can see is missing", and the API is still the refusal (PF-51).
 */
export function beyondOwnRights(
  effective: Effective | undefined,
  rules: GrantedRule[] | undefined,
): string[] {
  if (!effective || !Array.isArray(effective.grants) || effective.bootstrap === true) {
    return [];
  }
  const missing: string[] = [];
  for (const rule of rules ?? []) {
    const constraints = JSON.stringify(rule.constraints ?? []);
    for (const kind of rule.kinds ?? []) {
      for (const verb of rule.verbs ?? []) {
        const holds = effective.grants.some((grant) => {
          const held = grant.rule as GrantedRule;
          // `propose` holds `read`, as jc-core `Rule::grants` says and the server reads it.
          return (
            Boolean(held.kinds?.includes(kind)) &&
            Boolean(held.verbs?.includes(verb) || (verb === "read" && held.verbs?.includes("propose"))) &&
            (held.constraints ?? []).every((constraint) =>
              constraints.includes(JSON.stringify(constraint)),
            )
          );
        });
        const item = `${verb} on ${kind}`;
        if (!holds && !missing.includes(item)) {
          missing.push(item);
        }
      }
    }
  }
  return missing;
}

/** The kinds and the verbs the caller may grant in `project`, for the lists a role form offers. */
export function ownRights(effective: Effective | undefined): { kinds: string[]; verbs: string[] } {
  if (!effective || !Array.isArray(effective.grants) || effective.bootstrap === true) {
    return { kinds: [], verbs: [] };
  }
  const kinds = new Set<string>();
  const verbs = new Set<string>();
  for (const grant of effective.grants) {
    const rule = grant.rule as GrantedRule;
    for (const kind of rule.kinds ?? []) kinds.add(kind);
    for (const verb of rule.verbs ?? []) verbs.add(verb);
  }
  return { kinds: [...kinds].sort(), verbs: [...verbs].sort() };
}

/**
 * Whether to ask for a list of `kind`: `undefined` while the permissions are in flight, `false`
 * once they deny reading it or no binding covers the project (`permissions/me` answered 404, so the
 * list would 404 too, T-5754), and `true` otherwise — a document that never came for another
 * reason leaves the API to decide, as `allows` does.
 */
export function readsOnceKnown(
  permissions: { data: Effective | undefined; isLoading: boolean; denied?: boolean },
  kind: string,
) {
  if (permissions.denied === true) {
    return false;
  }
  const reads = mayRead(permissions.data, kind);
  return reads ?? (permissions.isLoading ? undefined : true);
}

/**
 * A list of `kind` in `project`, asked for only once the permissions say the caller reads it: the
 * API answers a list nobody granted with the 404 of an unknown one, which a page then showed as a
 * failure (T-3852). `hidden` is that answer known in advance; an unasked query stays idle and is
 * not pending.
 */
export function useReadableList(project: string, plural: string, kind: string, enabled = true) {
  const permissions = usePermissions(project);
  const reads = readsOnceKnown(permissions, kind);
  const query = useQuery({
    queryKey: queryKeys.list(project, plural),
    enabled: enabled && reads === true,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
  });
  // A list that is never asked for is not pending: a disabled query stays `isPending` for good,
  // which kept a page that waits on it at "Loading" (T-4027).
  return { ...query, isPending: query.isPending && enabled && reads !== false, hidden: reads === false };
}
