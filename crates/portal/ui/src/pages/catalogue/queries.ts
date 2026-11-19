import { api, unwrap } from "../../api/client";
import type { CatalogueSearch } from "./search";

/**
 * The catalogue's reads, kept out of the page chunks: the public router starts them as the route
 * matches, beside the page's own chunk, so a phone waits one round trip for both rather than the
 * chunk and then the read (T-5165).
 */
export function catalogueKey(search: CatalogueSearch) {
  return ["catalogue", search] as const;
}

export function datasetKey(name: string) {
  return ["catalogue", "datasets", name] as const;
}

export function catalogueQuery(search: CatalogueSearch) {
  return {
    // Access picks among datasets already read, so the server's answer is the same (T-7389).
    queryKey: catalogueKey({ ...search, access: undefined }),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/catalogue", {
          params: {
            query: {
              q: search.q,
              publisher: search.publisher,
              theme: search.theme,
              format: search.format,
              licence: search.licence,
              spatial: search.spatial,
              year: search.year,
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
