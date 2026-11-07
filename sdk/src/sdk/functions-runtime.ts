// The module jc-functions loads as `@joinedcontext/sdk/server`: the server entry, plus the client
// the runtime builds `ctx.jc` from over its one host request (Architecture/20 §3).
import { ProblemError } from "./client";
import type { FnHandler } from "./server";

export * from "./server";
export { createClient } from "./client";
export type { Cell, Row } from "../ngsi";
export type { DataClient, Feature, FeatureCollection, FeatureCollectionInfo, FeatureQuery, Query, TemporalPoint, TemporalQuery, TemporalRow } from "./client";
export { ProblemError };
export type { JcUser } from "./config";

/**
 * The Portal's entry around an App's function: a data call the caller may not make (`401`/`403`
 * from `ctx.jc`) answers that refusal, so the page says "no access" instead of "failed". Any
 * other throw stays the function's own failure. The caller's token is the only credential, so a
 * refusal here is the viewer's own and names nothing they could not read (T-4912).
 */
export function answeringRefusals(handler: FnHandler): FnHandler {
  return async (request, ctx) => {
    try {
      return await handler(request, ctx);
    } catch (err) {
      if (!(err instanceof ProblemError) || (err.status !== 401 && err.status !== 403)) throw err;
      return { status: err.status, body: { type: err.type, title: err.title, status: err.status, detail: err.detail } };
    }
  };
}
