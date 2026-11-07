import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";

export type OrganizationModel = components["schemas"]["OrganizationModel"];
export type CatalogueEntry = components["schemas"]["CatalogueEntry"];
export type OrganizationModels = components["schemas"]["OrganizationModels"];

/** The organization's model list, one cache entry per search (DM-63). */
export const organizationModelsKey = (search: string) => ["organizationModels", search] as const;

export function organizationModelsQuery(search: string) {
  return {
    queryKey: organizationModelsKey(search),
    queryFn: async (): Promise<OrganizationModels> =>
      unwrap(
        await api.GET("/api/v1/organization/datamodels", {
          params: { query: search === "" ? {} : { search } },
        }),
      ),
    staleTime: 30_000,
  };
}

/** `text` once the person stopped typing for `ms`, so a server search runs once per pause. */
export function useDebounced(text: string, ms = 250): string {
  const [settled, setSettled] = useState(text);
  useEffect(() => {
    const timer = setTimeout(() => {
      setSettled(text);
    }, ms);
    return () => {
      clearTimeout(timer);
    };
  }, [text, ms]);
  return settled;
}


/** Every model the caller may read; the search only adds catalogue entries. */
export function useOrganizationModels(search = "") {
  return useQuery(organizationModelsQuery(search.trim().length >= 2 ? search.trim() : ""));
}

/** The picker value of a model of the organization, and of a catalogue entry. */
export const modelValue = (model: Pick<OrganizationModel, "project" | "name">) => `${model.project}/${model.name}`;
export const catalogueValue = (entry: Pick<CatalogueEntry, "id">) => `sdm:${entry.id}`;
