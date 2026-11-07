import type { JcConfig } from "@joinedcontext/sdk";

/** Where one page reads its type: `endpoint` is set when the application reads several (SDK-02). */
export interface TypeSource {
  /** The page's id in the address (`#/{id}`): the type, or `{type}@{endpoint}` for a type several endpoints serve. */
  id: string;
  type: string;
  /** The endpoint the type is read and written through; undefined for an application of one endpoint. */
  endpoint?: string;
  /** More than one endpoint serves the type, so the page names its endpoint. */
  shared: boolean;
}

/**
 * One source per type, sorted; an application reading several endpoints reads each type through
 * the endpoint whose data needs name it, and a type several endpoints serve gets one source per
 * endpoint. A type no endpoint names is read through the primary, as the client does.
 */
export function sourcesOf(types: readonly string[], config: Pick<JcConfig, "endpoints">): TypeSource[] {
  const endpoints = config.endpoints ?? [];
  return [...types].sort().flatMap((type): TypeSource[] => {
    if (endpoints.length < 2) return [{ id: type, type, shared: false }];
    const serving = endpoints.filter((endpoint) => endpoint.types.includes(type));
    if (serving.length < 2) return [{ id: type, type, endpoint: (serving[0] ?? endpoints[0]).name, shared: false }];
    return serving.map((endpoint) => ({ id: `${type}@${endpoint.name}`, type, endpoint: endpoint.name, shared: true }));
  });
}
