import { useEffect, useState } from "react";
import { useInfiniteQuery } from "@tanstack/react-query";
import { api, queryKeys, unwrap, whilePending } from "../api/client";
import { asManifests } from "../api/manifest";

/** How many rows one read of a kind's list asks for. */
export const GRID_PAGE = 50;

/**
 * One kind's list as the server searches and pages it (API/13 section 2, T-6175): the grid's
 * search is `q`, sent once typing pauses, and Show more reads the next cursor. `server` is the
 * ManifestGrid prop as it is.
 */
export function useKindPages(project: string, plural: string) {
  const [typed, setTyped] = useState("");
  const [q, setQ] = useState("");
  useEffect(() => {
    const timer = window.setTimeout(() => setQ(typed.trim()), 250);
    return () => window.clearTimeout(timer);
  }, [typed]);
  const list = useInfiniteQuery({
    queryKey: [...queryKeys.list(project, plural), "pages", q],
    queryFn: async ({ pageParam }: { pageParam: string | undefined }) =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural }, query: { limit: GRID_PAGE, count: true, q: q || undefined, cursor: pageParam } },
        }),
      ),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.nextCursor ?? undefined,
    // A change on its way polls until it lands (T-1392).
    refetchInterval: (query) => whilePending({ state: { data: { items: query.state.data?.pages.flatMap((each) => each.items) ?? [] } } }),
  });
  const items = asManifests(list.data?.pages.flatMap((each) => each.items) ?? []);
  const server = {
    onSearch: setTyped,
    total: list.data?.pages.at(-1)?.totalCount ?? undefined,
    hasMore: list.hasNextPage,
    loadMore: () => void list.fetchNextPage(),
  };
  return { list, items, server };
}
