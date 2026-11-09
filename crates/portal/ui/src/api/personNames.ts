import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import i18n from "../i18n";
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

/**
 * How a page writes a person a manifest names: their name when it is known, "Unknown person"
 * for an id nobody resolved (a reader without the right, or a realm with no such user), never the
 * bare id in a page's text (UI-01). Anything that is not an id (an e-mail) is already words.
 * `PersonName` keeps the id on hover for whoever needs it.
 */
export function personLabel(id: string, names: Map<string, string>): string {
  const name = names.get(id);
  if (name) return name;
  return isPersonId(id) ? i18n.t("personName.unknown") : id;
}

/** Every person id inside a sentence (a server's message) written as `personLabel` writes it. */
export function withPersonNames(text: string, names: Map<string, string>): string {
  return text.replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, (id) =>
    personLabel(id, names),
  );
}
