import type { QueryClient } from "@tanstack/react-query";
import { api, queryKeys, unwrap } from "../../api/client";

/** One space's manifest, as its page reads it. */
export function spaceQuery(project: string, name: string) {
  return {
    queryKey: queryKeys.resource(project, "spaces", name),
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "spaces", name } },
        }),
      ),
  };
}

/** One kind's list in a project, as the space's page reads it. */
export function projectListQuery(project: string, plural: string) {
  return {
    queryKey: queryKeys.list(project, plural),
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural } },
        }),
      ),
  };
}

// Names are DNS labels (kinds: metadata.name): anything else is no space page to read ahead for.
const SPACE_PAGE = /^\/projects\/([a-z0-9-]{1,63})\/spaces\/([a-z0-9-]{1,63})\/?$/;
// The static pages under /spaces/ (router.tsx): reading one ahead as a space 404s (T-5749).
const NOT_A_SPACE = new Set(["new", "upload", "complete"]);

/**
 * A space's page opened from its address starts the reads its grid waits on with the session
 * check, not after it (T-5598): the space, the project's models (which name the grid's type) and
 * the page's code. On Slow 4G each read in turn cost a 0.6 s round trip before the first row. The
 * reads are the page's own, with the same session, so a person signed out gets the same refusal
 * the page would have got; the page reads them again when it is drawn.
 */
export function preloadSpacePage(client: QueryClient, pathname: string): void {
  const match = SPACE_PAGE.exec(pathname);
  if (!match || NOT_A_SPACE.has(match[2])) return;
  const [, project, name] = match;
  void client.prefetchQuery(spaceQuery(project, name));
  void client.prefetchQuery(projectListQuery(project, "datamodels"));
  void import("./SpaceInside");
}
