import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import { ORG_NAMESPACE } from "./manifest";
import { usePermissions } from "./permissions";

/** A Keycloak user id as a manifest names a person (ADR-N-061): a lower-case UUID. */
export function isPersonId(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
    value,
  );
}

/**
 * The names behind the person ids a page shows, for a reviewer who updates people or approves
 * groups (the server checks it again and answers 403 to anyone else). Everyone else, and every id
 * the realm does not know, sees the id: the map holds only what was resolved.
 */
export function usePersonNames(ids: readonly string[]): Map<string, string> {
  const permissions = usePermissions(ORG_NAMESPACE);
  // Asked only once the permissions are known (`allows` says yes while they load, T-5533).
  const grants = permissions.data?.grants;
  const grantsTo = (kind: string, verb: string) =>
    Array.isArray(grants) &&
    grants.some((grant) => {
      const rule = grant.rule as { kinds?: string[]; verbs?: string[] };
      return (
        (rule.kinds?.includes(kind) || rule.kinds?.includes("*")) &&
        Boolean(rule.verbs?.includes(verb))
      );
    });
  const resolves =
    permissions.data?.bootstrap === true ||
    grantsTo("Person", "update") ||
    grantsTo("Group", "approve");
  const wanted = [...new Set(ids.filter(isPersonId))]
    .sort()
    .slice(0, 100)
    .join(",");
  const names = useQuery({
    queryKey: ["people", "names", wanted],
    enabled: resolves && wanted !== "",
    retry: false,
    staleTime: 60_000,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/organization/people/names", {
          params: { query: { ids: wanted } },
        }),
      ),
  });
  return useMemo(
    () =>
      new Map((names.data?.items ?? []).map((item) => [item.id, item.name])),
    [names.data],
  );
}

/** How a page writes a person a manifest names: their name beside the id when it is known. */
export function personLabel(id: string, names: Map<string, string>): string {
  const name = names.get(id);
  return name ? `${name} (${id})` : id;
}
