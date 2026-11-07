import { unwrap } from "./client";

export interface GatewayEntity {
  id: string;
  type: string;
  name?: string;
}

export async function searchEntities(params: {
  space: string;
  type: string;
  q?: string;
  limit?: number;
  signal?: AbortSignal;
}): Promise<GatewayEntity[]> {
  const query = new URLSearchParams();
  query.set("type", params.type);
  query.set("limit", String(params.limit ?? 20));

  const trimmedQ = params.q?.trim();
  if (trimmedQ) {
    // `~=` is a regular expression: the typed text is searched as written, so its metacharacters
    // are escaped first (F-UI1-020). NGSI-LD then quotes the pattern, so a quote or backslash in
    // it would end the string early.
    const literal = trimmedQ.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const escaped = literal.replace(/[\\"]/g, (c) => `\\${c}`);
    query.set("q", `name~="${escaped}"`);
  }

  const url = `/cs/${encodeURIComponent(params.space)}/ngsi-ld/v1/entities?${query.toString()}`;

  const response = await fetch(url, {
    method: "GET",
    credentials: "same-origin",
    headers: {
      Accept: "application/ld+json",
    },
    signal: params.signal,
  });

  if (!response.ok) {
    const error = await response.json().catch(() => undefined);
    await unwrap({ error, response });
  }

  const data: unknown = await response.json();
  if (!Array.isArray(data)) {
    return [];
  }

  return data
    .filter((item): item is Record<string, unknown> => typeof item === "object" && item !== null)
    .map((entity) => {
      const nameVal = (entity.name as { value?: unknown } | undefined)?.value;
      return {
        id: typeof entity.id === "string" ? entity.id : "",
        type: typeof entity.type === "string" ? entity.type : "",
        name: typeof nameVal === "string" ? nameVal : undefined,
      };
    })
    .filter((entity) => entity.id !== "");
}

/** One entity type of a space as its broker lists it, with the attribute names its entities carry. */
export interface SpaceType {
  type: string;
  attributes: string[];
}

/**
 * The entity types of a space the caller may read (NGSI-LD `/types?details=true`, CIM 009
 * 5.7.6): the gateway answers through the caller's Policy, so a type they may not read is not
 * listed. For a space whose data model declares no type, this is where the choices come from.
 */
export async function listSpaceTypes(space: string, signal?: AbortSignal): Promise<SpaceType[]> {
  const response = await fetch(`/cs/${encodeURIComponent(space)}/ngsi-ld/v1/types?details=true`, {
    method: "GET",
    credentials: "same-origin",
    headers: { Accept: "application/ld+json" },
    signal,
  });
  if (!response.ok) {
    const error = await response.json().catch(() => undefined);
    await unwrap({ error, response });
  }
  const data: unknown = await response.json();
  const items: unknown[] = Array.isArray(data)
    ? data
    : Array.isArray((data as { typeList?: unknown } | null)?.typeList)
      ? ((data as { typeList: unknown[] }).typeList)
      : [];
  return items
    .map((item): SpaceType | undefined => {
      if (typeof item === "string") return { type: item, attributes: [] };
      const entry = item as { typeName?: unknown; attributeNames?: unknown } | null;
      if (typeof entry?.typeName !== "string") return undefined;
      const names = Array.isArray(entry.attributeNames) ? entry.attributeNames.filter((n): n is string => typeof n === "string") : [];
      return { type: entry.typeName, attributes: names };
    })
    .filter((entry): entry is SpaceType => entry !== undefined && entry.type !== "");
}
