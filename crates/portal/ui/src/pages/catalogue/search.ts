/**
 * The catalogue's search as the address carries it (EP-81): `?q=…&publisher=…&page=2`, so a
 * filtered list is a link a person can hand to someone else.
 */
export const FACETS = ["publisher", "theme", "format", "licence", "spatial", "year"] as const;
export type FacetName = (typeof FACETS)[number];

export interface CatalogueSearch {
  q?: string;
  publisher?: string[];
  theme?: string[];
  format?: string[];
  licence?: string[];
  spatial?: string[];
  year?: string[];
  page?: number;
}

function strings(value: unknown): string[] | undefined {
  const list = (Array.isArray(value) ? value : [value]).filter(
    (item): item is string => typeof item === "string" && item.trim() !== "",
  );
  return list.length > 0 ? [...new Set(list)] : undefined;
}

/** Reads the router's search into the catalogue's; anything unexpected is dropped, never thrown. */
export function parseCatalogueSearch(search: Record<string, unknown>): CatalogueSearch {
  const out: CatalogueSearch = {};
  if (typeof search.q === "string" && search.q.trim() !== "") {
    out.q = search.q.slice(0, 200);
  }
  for (const facet of FACETS) {
    const values = strings(search[facet]);
    if (values) {
      out[facet] = values;
    }
  }
  const page = Number(search.page);
  if (Number.isInteger(page) && page > 1) {
    out.page = page;
  }
  return out;
}

/** The search with one facet value turned on or off; any change of filter starts at page one. */
export function toggled(search: CatalogueSearch, facet: FacetName, value: string): CatalogueSearch {
  const current = search[facet] ?? [];
  const next = current.includes(value) ? current.filter((v) => v !== value) : [...current, value];
  return { ...search, [facet]: next.length > 0 ? next : undefined, page: undefined };
}

/** Whether any filter or text narrows the list. */
export function narrowed(search: CatalogueSearch): boolean {
  return Boolean(search.q) || FACETS.some((facet) => (search[facet]?.length ?? 0) > 0);
}

/**
 * A dataset's data view as the address carries it (T-7072): `?type=…&q=…&search=…&sort=…`, as the
 * endpoint page keeps them (T-5082), so a filtered, ordered table is a link a visitor hands on. The
 * search box's words travel apart from the query (T-7112), so the reopened box shows them. Only the
 * view travels: the read stays the opener's own session, never a token in the address.
 */
export function parseDataViewSearch(search: Record<string, unknown>): { type?: string; q?: string; search?: string; sort?: string } {
  const text = (value: unknown, max: number) => (typeof value === "string" && value.trim() !== "" ? value.slice(0, max) : undefined);
  return { type: text(search.type, 200), q: text(search.q, 2000), search: text(search.search, 200), sort: text(search.sort, 200) };
}
