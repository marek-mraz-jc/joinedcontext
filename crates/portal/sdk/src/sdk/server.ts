export type { Cell, Row } from "../ngsi";
export type { DataClient, Feature, FeatureCollection, FeatureCollectionInfo, FeatureQuery, Query, TemporalPoint, TemporalQuery, TemporalRow } from "./client";
export { ProblemError } from "./client";
export type { JcUser } from "./config";
import type { JcUser } from "./config";

export interface FnRequest {
  method: "GET" | "POST";
  query: Record<string, string>;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  body: any;
  user: JcUser | null;
}

export interface FnResponse {
  status?: number;
  body?: unknown;
}

export interface FnContext {
  jc: import("./client").DataClient;
  log(...parts: unknown[]): void;
}

export type FnHandler = (request: FnRequest, ctx: FnContext) => Promise<FnResponse>;

/**
 * The `403` a function answers when its caller does not hold `role` of the App, `undefined` when
 * they do (SDK-41, AP-109). `request.user` is the person the Portal verified for this call, never
 * what a browser says, so the check holds even against a patched page:
 * `const refused = requireRole(request, "steward"); if (refused) return refused;`.
 */
export function requireRole(request: Pick<FnRequest, "user">, role: string): FnResponse | undefined {
  if (request.user?.roles?.includes(role)) return undefined;
  const detail = request.user ? `this needs the App's role '${role}'` : "this needs a signed-in person";
  return {
    status: request.user ? 403 : 401,
    body: {
      type: "https://joinedcontext.com/errors/role-required",
      title: request.user ? "Role Required" : "Sign In Required",
      status: request.user ? 403 : 401,
      detail,
      role,
    },
  };
}
