/**
 * The caller's access on an Endpoint, as the gateway decides it (EP-55, EP-56, T-5528). The
 * browser renders these answers and computes none of its own: each is read with the person's
 * session, so it is the decision their real request gets.
 */
import { useQuery } from "@tanstack/react-query";
import { endpointSource, originTransport, SourceError } from "@joinedcontext/sdk";
import type { EntityGridProps, EntitySource } from "@joinedcontext/sdk";
import { ApiError, api, unwrap } from "./client";

// Read off the SDK's exported shapes: each name the SDK exports costs the app builder's prompt.
/** One attribute's decisions: `{name, read, write, delete, reason}`. */
export type AttributeAccess = Awaited<ReturnType<NonNullable<EntitySource["access"]>>>[number];
type Explain = NonNullable<EntityGridProps["explainAccess"]>;
export type AccessAction = Parameters<Explain>[1];
export type AccessExplanation = Awaited<ReturnType<Explain>>;

export interface GeoQ {
  geometry?: string;
  coordinates?: unknown;
  georel?: string;
}

/** One entry of the gateway's AuthZEN permissions document (API/02 §7b). */
export interface GrantEntry {
  resource?: { type?: string; id?: string; idPatterns?: string[] };
  actions?: string[];
  attributes?: string[] | "*";
  constraints?: { q?: string; scopeQ?: string; geoQ?: GeoQ; temporalQ?: unknown };
}

/**
 * The AuthZEN grant document as the Context Gateway serves it today (EP-55, EP-56): one entry
 * per entity type a policy names, with the operations, the attribute projection and the residual
 * the gateway would add to any request. A type no grant names is absent, never listed as denied
 * (EP-59), so an empty matrix means "nothing here for you", not "nothing exists".
 */
export interface GrantDocument {
  subject?: { type?: string; id?: string };
  resource?: { type?: string; id?: string; space?: string };
  permissions?: GrantEntry[];
  prohibitions?: GrantEntry[];
}

/** A constraint as one line of text: a geometry or a temporal query arrives as an object. */
export function constraintText(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value);
}

/** A read of the gateway under `/api/endpoint/{slug}`, with the person's session. */
export async function gatewayFetch(slug: string, path: string, init?: RequestInit): Promise<Response> {
  // A `Request` rather than a URL string, as `api/client.ts` sends: same origin, the user's session.
  const response = await globalThis.fetch(
    new Request(new URL(`/api/endpoint/${encodeURIComponent(slug)}${path}`, window.location.origin), {
      credentials: "same-origin",
      ...init,
      headers: { Accept: "application/json", ...(init?.headers ?? {}) },
    }),
  );
  if (!response.ok) {
    throw new ApiError(response.status, `HTTP ${response.status}`);
  }
  return response;
}

/** The person's decisions per attribute of `type` on one Endpoint (API/02 §7b). */
export function attributeAccessQuery(slug: string | undefined, type: string | undefined) {
  return {
    queryKey: ["attribute-access", slug, type],
    enabled: Boolean(slug && type),
    retry: false,
    // One person's decisions never outlive their page (R17).
    gcTime: 0,
    queryFn: async (): Promise<AttributeAccess[]> => {
      // The SDK's own reading, so the grid and this page trust the same rows (deny by default).
      // A `Request` on the page's origin, as gatewayFetch sends it.
      const send = originTransport((path, init) => globalThis.fetch(new Request(new URL(String(path), window.location.origin), init)));
      const access = endpointSource(slug!, send).access;
      try {
        return await access!(type!);
      } catch (error) {
        throw error instanceof SourceError ? new ApiError(error.status, error.message) : error;
      }
    },
  };
}

export function useAttributeAccess(slug: string | undefined, type: string | undefined) {
  return useQuery(attributeAccessQuery(slug, type));
}

/**
 * The endpoint's own decision on one action over `type` (AuthZEN check, API/02 §7b), for a control
 * that is offered only when the server says yes (T-5530). Anything but a yes refuses.
 */
export function useAccessCheck(slug: string | undefined, type: string | undefined, action: string) {
  return useQuery({
    queryKey: ["access-decision", slug, type, action],
    enabled: Boolean(slug && type),
    retry: false,
    gcTime: 0,
    queryFn: async (): Promise<boolean> => {
      const response = await gatewayFetch(slug!, "/access/check", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ action: { name: action }, resource: { type } }),
      });
      const body = (await response.json()) as { decision?: unknown };
      return body.decision === true;
    },
  });
}

/** The attributes of `shown` the decisions do not let the person read: none read yet is all of them. */
export function unreadableOf(rows: AttributeAccess[] | undefined, shown: readonly string[]): string[] {
  const readable = new Set((rows ?? []).filter((row) => row.read).map((row) => row.name));
  return shown.filter((name) => !readable.has(name));
}

/** The attributes the decisions let the person correct in place: a write, and not a formula's. */
export function writableOf(rows: AttributeAccess[] | undefined): string[] {
  return (rows ?? []).filter((row) => row.write && row.reason !== "formula").map((row) => row.name);
}

/** The attributes the person may not read, each with the words of its reason. */
export function deniedOf(rows: AttributeAccess[] | undefined, t: (key: string) => string): Record<string, string> {
  return Object.fromEntries(
    (rows ?? [])
      .filter((row) => !row.read)
      .map((row) => [row.name, t(row.reason === "prohibited" ? "access.panel.attrProhibited" : "access.panel.attrDenied")]),
  );
}

/** The Policy behind a refused attribute, named only to a caller who may read it (API/01 §58). */
export async function explainAttribute(
  project: string,
  endpoint: string,
  type: string,
  attribute: string,
  action: AccessAction,
): Promise<AccessExplanation> {
  const answer = await unwrap(
    await api.GET("/api/v1/projects/{project}/endpoints/{name}/access/attributes", {
      params: { path: { project, name: endpoint }, query: { type, attribute, action } },
    }),
  );
  return { policy: answer.policy ?? null, policyTitle: answer.policyTitle ?? null, policyHidden: answer.policyHidden };
}
