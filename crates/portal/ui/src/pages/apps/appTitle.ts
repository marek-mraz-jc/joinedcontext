import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { asManifests, localized } from "../../api/manifest";

/**
 * What an application is called on screen: never its id. A run or manifest with a title shows
 * the title; without one the name is read as words ("map-visualization" → "Map visualization")
 * and the endpoint's title, when known, follows it. The raw name stays for URLs and details.
 */

/** "map-visualization" → "Map visualization". */
export function humanizeName(name: string): string {
  const words = name.replace(/[-_.]+/g, " ").replace(/\s+/g, " ").trim().toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

export function appDisplayName({
  title,
  appName,
  endpointTitle,
}: {
  title?: string | null;
  appName?: string | null;
  endpointTitle?: string | null;
}): string {
  const own = title?.trim();
  if (own) {
    return own;
  }
  const name = humanizeName(appName ?? "");
  const endpoint = endpointTitle?.trim();
  return name && endpoint ? `${name} · ${endpoint}` : name || endpoint || "";
}

/** The project's endpoint titles by name, in the reader's language; an untitled endpoint has none. */
export function useEndpointTitles(project: string): Map<string, string> {
  const { i18n } = useTranslation();
  const query = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });
  return useMemo(() => {
    const titles = new Map<string, string>();
    for (const endpoint of asManifests(query.data?.items ?? [])) {
      const title = localized(endpoint.metadata.title, i18n.language, "");
      if (title) {
        titles.set(endpoint.metadata.name, title);
      }
    }
    return titles;
  }, [query.data, i18n.language]);
}
