import { api, unwrap } from "../../api/client";
import type { CatalogueSearch } from "./search";

/**
 * The catalogue's reads, kept out of the page chunks: the public router starts them as the route
 * matches, beside the page's own chunk, so a phone waits one round trip for both rather than the
 * chunk and then the read (T-5165).
 */
export function catalogueKey(search: CatalogueSearch & { project?: string; signedIn?: boolean }) {
  return ["catalogue", search] as const;
}

export function datasetKey(name: string) {
  return ["catalogue", "datasets", name] as const;
}

/**
 * One page of the catalogue; with `project`, that project's datasets alone (T-7389). Signed in,
 * the answer also holds the internal datasets granted to the person (T-7400): it is asked past
 * the browser's cache, which may still hold the public answer from before the sign-in.
 */
export function catalogueQuery(search: CatalogueSearch, project?: string, signedIn = false) {
  return {
    queryKey: catalogueKey({ ...search, scope: undefined, ...(project ? { project } : {}), ...(signedIn ? { signedIn } : {}) }),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/catalogue", {
          ...(signedIn ? { cache: "no-store" as const } : {}),
          params: {
            query: {
              q: search.q,
              publisher: search.publisher,
              theme: search.theme,
              format: search.format,
              licence: search.licence,
              spatial: search.spatial,
              year: search.year,
              project,
              page: search.page,
            },
          },
        }),
      ),
  };
}

export function datasetQuery(name: string) {
  return {
    queryKey: datasetKey(name),
    queryFn: async () => unwrap(await api.GET("/api/v1/catalogue/datasets/{name}", { params: { path: { name } } })),
  };
}

/** What the signed-in person may read, decided per Endpoint by the gateway (T-6411). */
export function readableQuery() {
  return {
    queryKey: ["catalogue", "readable"] as const,
    queryFn: async () => unwrap(await api.GET("/api/v1/catalogue/readable")),
  };
}
